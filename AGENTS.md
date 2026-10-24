<!-- g360:agents:begin -->
# AGENTS.md — g360-ventas-db

Solución de datos orientada al procesamiento y generación de una base de datos derivada a partir de información corporativa de ventas, aislada de la fuente orig

> El bloque entre los markers `g360:agents` lo genera `g360 agents` y se
> regenera sin perder lo que escribas afuera.

## Verificación

Un solo comando deja el repo listo y verifica que todo esté en verde:

```bash
cargo build && cargo test
```

- Stack: rust
- Tests: cargo test en `tests/`

## Estructura

- `src/`
- `tests/`
- `frontend/`
- `scripts/`
- `data/`
- `src-tauri`

## Convenciones

- `cargo fmt` y `cargo clippy` antes de commitear; `cargo test` antes de entregar.
- Comentarios que expliquen el *por qué*, no el *qué*.

## Reglas de este repo

- **Autosuficiente**: `git clone` + el comando de verificación debe dejar la app levantada y los tests en verde, sin red y sin ayuda de una persona.
- **Tests honestos**: nada de `it.skip`, `pytest.mark.skip`, `xfail` ni `assert true` para forzar verde.
- **Entorno declarativo**: toda variable de entorno va en `.env.example` con un default seguro.
- **Sin paths de tu máquina**: nada de rutas absolutas locales ni secretos en el código.
- **Evidencia**: al entregar, reportá el comando que corriste y su salida. Si no pudiste verificar, decilo.
<!-- g360:agents:end -->
