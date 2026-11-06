#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
mod commands;
mod state;
mod types;

use std::env;
use std::sync::Arc;

use g360_db_ventas::capture_state::{CapturePhase, SharedProgress};
use state::CAPTURE_STATE;
use commands::*;

/// Ejecuta el pipeline diario y sale. Usado por Task Scheduler.
async fn run_auto_sync_and_exit() -> ! {
    use chrono::{Local, TimeDelta};
    
    let cfg = g360_db_ventas::config::load_config();
    
    // Verificar configuración mínima
    if !cfg.supabase.is_configured() {
        eprintln!("ERROR: Supabase no configurado");
        std::process::exit(1);
    }
    if cfg.intranet.user.is_empty() || cfg.intranet.pass.is_empty() {
        eprintln!("ERROR: Credenciales de intranet no configuradas");
        std::process::exit(1);
    }
    
    println!("Iniciando sync automático...");

    let pool = match g360_db_ventas::db::writer::init_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ERROR: No se pudo conectar a la BD: {}", e);
            std::process::exit(1);
        }
    };

    // Paso 1: Capturar desde (MAX(fecha_orig) - 1) hasta hoy.
    // Mismo buffer que el botón manual: si la PC estuvo apagada el fin de semana
    // (o varios días), el ERP conserva los datos y este rango los recupera todo.
    // Capturar SOLO "ayer" perdería los días intermedios del gap.
    println!("Paso 1: Capturando desde el último día registrado (buffer -1)...");
    let start = {
        let last: Option<String> = sqlx::query_scalar("SELECT MAX(fecha_orig) FROM ventas")
            .fetch_one(&pool)
            .await
            .unwrap_or(None);
        match last {
            Some(d) => {
                if let Ok(dt) = chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d") {
                    (dt - chrono::Duration::days(1)).format("%Y-%m-%d").to_string()
                } else {
                    d
                }
            }
            None => {
                // BD vacía: capturar solo el día anterior (primer arranque)
                (Local::now().naive_local().date() - TimeDelta::days(1)).to_string()
            }
        }
    };
    println!("  Rango: {} -> hoy", start);

    let capture_result = g360_db_ventas::capture::run_batch_history(&start, "", false, CAPTURE_STATE.clone(), None, 3).await;

    match capture_result {
        Ok(_) => {
            println!("✓ Captura completada: {}", start);
            
            // Paso 2: Verificar integridad
            println!("Paso 2: Verificando integridad...");
            if let Ok(issues) = g360_db_ventas::db::writer::verify_integrity(&pool).await {
                for issue in &issues {
                    if issue.contains("⚠") || issue.contains("ERROR") {
                        eprintln!("  {}", issue);
                    }
                }
            }
            
            // Paso 3: Reconciliar con Supabase (mes por mes, resumible) — opcional
            if !cfg.supabase_enabled || !cfg.supabase.is_configured() {
                println!("Supabase deshabilitado o no configurado: solo captura local.");
                let _ = g360_db_ventas::db::writer::calculate_monthly_checksums(&pool).await;
                println!("\n=== SYNC LOCAL COMPLETADO ===");
                std::process::exit(0);
            }
            println!("Paso 3: Reconciliando con Supabase (mes por mes)...");
            let shared: SharedProgress = CAPTURE_STATE.clone();
            let progress_cb: g360_db_ventas::processor::uploader::ProgressCb = Some(Arc::new(move |_b, _t, pct, msg| {
                let mut s = shared.lock().unwrap();
                s.phase = CapturePhase::Uploading;
                s.message = msg.to_string();
                s.progress = pct;
                drop(s);
            }));
            match g360_db_ventas::processor::uploader::reconcile_supabase(
                &pool,
                cfg.supabase_retention_days_effective(),
                &progress_cb,
            ).await {
                Ok(res) => {
                    println!(
                        "✓ Reconciliación: {} meses sincronizados, {} fallidos, {} filas subidas, {} purgadas{}",
                        res.months_synced, res.months_failed, res.rows_uploaded, res.rows_purged,
                        if res.stopped_early { " (cortado; se reanuda luego)" } else { "" }
                    );
                    // Calcular checksums
                    println!("Paso 4: Calculando checksums...");
                    if let Ok(_) = g360_db_ventas::db::writer::calculate_monthly_checksums(&pool).await {
                        println!("✓ Checksums calculados");
                    }
                    println!("\n=== SYNC AUTOMÁTICO COMPLETADO ===");
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("ERROR: Fallo en reconciliación: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("ERROR: Fallo en captura: {}", e);
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    
    // Verificar si se ejecuta desde Task Scheduler
    let is_task_scheduler = args.iter().any(|a| a == "--task-scheduler" || a == "--auto-sync");
    
    if is_task_scheduler {
        // Ejecutar sync y salir
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(run_auto_sync_and_exit());
    } else {
        // Modo normal: ejecutar app Tauri
        #[cfg(not(debug_assertions))]
        {
            use tracing_subscriber::{fmt::Layer, EnvFilter};
            use std::fs::OpenOptions;
            use std::io::Write;
            
            let log_dir = g360_db_ventas::config::logs_dir();
            let _ = std::fs::create_dir_all(&log_dir);
            // Rotacion simple: si app.log supera 50 MB, renombrar a app.log.old
            // (solo se conserva una generacion; el disco no crece infinito)
            let log_path = log_dir.join("app.log");
            if let Ok(meta) = std::fs::metadata(&log_path) {
                if meta.len() > 50 * 1024 * 1024 {
                    let old = log_dir.join("app.log.old");
                    let _ = std::fs::remove_file(&old);
                    let _ = std::fs::rename(&log_path, &old);
                }
            }
            if let Ok(f) = OpenOptions::new().create(true).append(true).open(&log_path) {
                tracing_subscriber::fmt()
                    .with_env_filter(EnvFilter::new("info"))
                    .with_writer(std::sync::Mutex::new(f))
                    .with_ansi(false)
                    .init();
            }
        }
        
        tauri::Builder::default()
            .invoke_handler(tauri::generate_handler![
                get_dashboard, get_health, get_settings, save_settings,
                test_supabase, get_supabase_info, get_local_metrics, upload_all, admin_reset, reset_sync_marker, save_service_role_key,
                capture_range, sync_from_last, sync_smart, reconcile_supabase_cmd, run_full_flow,
                clear_cache, open_data_folder, clear_db,
                get_capture_status,
                list_months, delete_month, delete_months, get_completeness, get_failed_days, import_manual_day, import_manual_month,
                preview_import, reparse_raw,
                abort_capture, test_intranet, get_credentials_status, preview_csv,
                // Auditoría e integridad
                verify_integrity, calculate_checksums, get_sync_history, get_checksum_history,
                get_data_quality, export_nc_ndb, get_monthly_monitor, export_monitor_xlsx,
                // Protección de upload
                upload_dry_run, upload_all_with_verify,
                // Mantenimiento DB
                vacuum_db, backup_db, export_full_db, export_canonical_snapshot, recalculate_checksums, get_db_health,
                check_contract_version, get_contract_metadata,
                // Automatización diaria
                auto_daily_pipeline, get_next_capture_time,
                // Normalización de IDs para búsquedas
                normalize_query_id_cmd,
                // Último cartucho generado
                get_last_cartucho,
                // Wizard primer inicio
                is_first_run, save_first_config,
                // ZIP cartucho + recordatorio backup
                zip_cartucho_cmd, get_backup_status,
                // Tarea programada (Task Scheduler)
                get_scheduled_task, create_scheduled_task, delete_scheduled_task
            ])
            .run(tauri::generate_context!())
            .expect("error while running tauri application");
    }
}

