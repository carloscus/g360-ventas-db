# auto_deploy_check.ps1
# Ejecutar en PC nueva antes de la primera captura

Write-Host "=== VERIFICACIÓN DE DEPLOY ===" -ForegroundColor Cyan

# 1. Verificar credenciales
$config = "$env:APPDATA\g360-db-ventas\data\config.json"
if (Test-Path $config) {
    $cfg = Get-Content $config -Raw | ConvertFrom-Json
    if ($cfg.supabase.url -match "TU_SUPABASE") {
        Write-Host "❌ ERROR: Supabase no configurado" -ForegroundColor Red
        exit 1
    }
    if ($cfg.allowed_lines.Count -eq 0) {
        Write-Host "❌ ERROR: allowed_lines vacío" -ForegroundColor Red
        exit 1
    }
    Write-Host "✅ Credenciales OK" -ForegroundColor Green
} else {
    Write-Host "❌ ERROR: config.json no encontrado" -ForegroundColor Red
    exit 1
}

# 2. Verificar BD existe
$db = "$env:APPDATA\g360-db-ventas\data\historial.db"
if (-not (Test-Path $db)) {
    Write-Host "ℹ️  BD no existe (primera vez) - se creará automáticamente" -ForegroundColor Yellow
} else {
    $size = (Get-Item $db).Length / 1MB
    Write-Host "✅ BD existe ($([math]::Round($size,1)) MB)" -ForegroundColor Green
}

# 3. Verificar raw directory
$raw = "$env:APPDATA\g360-db-ventas\data\raw"
if (Test-Path $raw) {
    $csvCount = (Get-ChildItem $raw -Filter "*.csv" | Measure-Object).Count
    Write-Host "✅ Raw directory OK ($csvCount CSVs)" -ForegroundColor Green
} else {
    Write-Host "ℹ️  Raw directory vacío (captura pendiente)" -ForegroundColor Yellow
}

# 4. Verificar lock
$lock = "$raw\capture.lock"
if (Test-Path $lock) {
    $lockAge = (Get-Item $lock).LastWriteTime
    if ((Get-Date) - $lockAge -gt (New-TimeSpan -Hours 1)) {
        Write-Host "⚠️  Lock antiguo detectado - eliminando" -ForegroundColor Yellow
        Remove-Item $lock -Force
    }
}

Write-Host "`n=== DEPLOY VERIFICADO ===" -ForegroundColor Cyan
Write-Host "Listo para capturar. Rango recomendado: 2014-01 hasta hoy" -ForegroundColor Green
