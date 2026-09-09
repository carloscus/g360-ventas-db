# Backup semanal de historial.db (G360 Ventas) a Documentos\G360_Backups
# Mantiene las ultimas 4 copias. Usa sqlite3 .backup via VACUUM INTO (consistente
# incluso si la app esta corriendo, gracias a WAL).
# Ejecutar con: powershell -NoProfile -ExecutionPolicy Bypass -File backup_g360.ps1

$ErrorActionPreference = 'Stop'
$db  = Join-Path $env:APPDATA 'g360-db-ventas\data\historial.db'
$dst = Join-Path ([Environment]::GetFolderPath('MyDocuments')) 'G360_Backups'

if (-not (Test-Path $db))  { Write-Output "ERROR: no existe $db";  exit 1 }
if (-not (Test-Path $dst)) { New-Item -ItemType Directory -Path $dst -Force | Out-Null }

$stamp  = Get-Date -Format 'yyyyMMdd_HHmm'
$target = Join-Path $dst "historial_$stamp.db"

# VACUUM INTO: copia consistente y compactada (sin WAL/shm), segura con la app abierta
$py = @"
import sqlite3, sys
src, dst = sys.argv[1], sys.argv[2]
con = sqlite3.connect(src)
con.execute("VACUUM INTO ?", (dst,))
con.close()
print('ok')
"@
$tmpPy = Join-Path $env:TEMP "g360_vacuum_into.py"
Set-Content -Path $tmpPy -Value $py -Encoding UTF8

python $tmpPy $db $target
if ($LASTEXITCODE -ne 0) { Write-Output "ERROR: VACUUM INTO fallo"; exit 1 }

$sizeMB = [math]::Round((Get-Item $target).Length / 1MB, 0)
Write-Output "Backup creado: $target ($sizeMB MB)"

# Retencion: quedarse solo con las 4 mas recientes
Get-ChildItem $dst -Filter 'historial_*.db' |
    Sort-Object LastWriteTime -Descending |
    Select-Object -Skip 4 |
    ForEach-Object { Remove-Item $_.FullName -Force; Write-Output "Eliminado backup viejo: $($_.Name)" }
