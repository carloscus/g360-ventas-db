// Test completo: credenciales intranet -> descarga -> verificacion
use g360_db_ventas::browser::http::CipsaHttp;
use g360_db_ventas::config::{effective_intranet_user, load_dotenv};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .init();
    load_dotenv();

    let user = effective_intranet_user();
    println!("=== TEST COMPLETO CREDENCIALES ===\n");
    println!("usuario: '{}'", user);

    // Test 1: login real
    println!("\n[1] Login con credenciales reales...");
    let http = CipsaHttp::new()?;
    match http.login().await {
        Ok(_) => println!("    OK: credenciales validas, sesion activa"),
        Err(e) => { println!("    FAIL: {e:#}"); return Ok(()); }
    }

    // Test 2: password incorrecto
    println!("\n[2] Login con password INCORRECTO...");
    let client = reqwest::Client::builder()
        .cookie_store(true).danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(30)).build()?;
    let html = client.get("http://intranet.cipsa.com.pe/intranetcipsa/login.aspx").send().await?.text().await?;
    let mut form: Vec<(String,String)> = Vec::new();
    for m in html.match_indices("<input") {
        let rest = &html[m.0..(m.0+html[m.0..].len().min(200_000))];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        let name = tag.find("name=\"").and_then(|i| { let s=&tag[i+6..]; s.find('"').map(|j| &s[..j]) }).unwrap_or("");
        if !name.starts_with("__") { continue; }
        let val = tag.find("value=\"").and_then(|i| { let s=&tag[i+7..]; s.find('"').map(|j| &s[..j]) }).unwrap_or("");
        form.push((name.to_string(), val.to_string()));
    }
    form.push(("txtnombre".into(), user.clone()));
    form.push(("txtpass".into(), "PASSWORD_INVALIDO_XYZ".into()));
    form.push(("Button1".into(), "Aceptar".into()));
    let resp = client.post("http://intranet.cipsa.com.pe/intranetcipsa/login.aspx")
        .form(&form).timeout(std::time::Duration::from_secs(30)).send().await?;
    let body = resp.text().await?;
    let still_login = body.contains("txtnombre");
    println!("    status: {} | rechazado: {}", resp.status().as_u16(), still_login);
    if still_login { println!("    OK: rechaza credenciales invalidas correctamente"); }
    else { println!("    WARN: acepto credenciales invalidas"); }

    // Test 3: descarga 1 dia
    println!("\n[3] Descarga HTTP 1 dia...");
    let http2 = CipsaHttp::new()?;
    http2.login().await?;
    let range = g360_db_ventas::browser::captor::MonthRange {
        start: chrono::NaiveDate::from_ymd_opt(2026,1,15).unwrap(),
        end: chrono::NaiveDate::from_ymd_opt(2026,1,15).unwrap(),
        label: "2026-01-15".into(),
    };
    let raw = g360_db_ventas::config::raw_dir();
    std::fs::create_dir_all(&raw)?;
    match tokio::time::timeout(std::time::Duration::from_secs(60), http2.download(&range, &raw)).await {
        Ok(Ok(p)) => { let sz = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0); println!("    OK: {} bytes", sz); },
        Ok(Err(e)) => println!("    FAIL: {e:#}"),
        Err(_) => println!("    TIMEOUT"),
    }

    println!("\n=== TEST COMPLETADO ===");
    Ok(())
}
