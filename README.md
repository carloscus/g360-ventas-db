# G360 DB Ventas

Captura automática de ventas del intranet CIPSA → SQLite local + Supabase.

## Arquitectura

- **`src/`** — núcleo Rust (`g360-db-ventas`):
  - `browser/captor.rs` — login y descarga de exportaciones XLS desde el intranet.
  - `processor/` — normalización (`.xls` → CSV), parsing y subida a Supabase.
  - `db/` — esquema, escritura y consultas sobre SQLite.
- **`src-tauri/`** — envoltorio Tauri v2 con el frontend en `frontend/` (frontend estático, `index.html` + `logo_cipsa.svg`).
- **`data/`** — datos de la empresa (no versionados; ver `.gitignore`).

## Varios binarios (CLI)

```
cargo run --bin capture      # captura 1 mes
cargo run --bin batch        # captura N meses
cargo run --bin normalize    # .xls -> CSV -> SQLite
cargo run --bin upload       # SQLite -> Supabase
cargo run --bin query        # consulta la BD
```

## Variables de entorno

Las credenciales de acceso al intranet se leen desde variables de entorno (no están
hardcodeadas en el código). Copia `.env.example` a `.env` y definelas antes de ejecutar:

```
G360_INTRANET_USER=ccusi
G360_INTRANET_PASS=tu_password
```

Para la app de escritorio (Tauri), define las variables antes de lanzar el binario:

```
# PowerShell
$env:G360_INTRANET_USER="ccusi"; $env:G360_INTRANET_PASS="..." ; .\tauri\dist\...  o  cargo tauri dev
```

La configuración de Supabase (URL/Key) se guarda en `config.json` dentro del directorio
de datos de la app (`%APPDATA%\g360-db-ventas\data\config.json`).