# RV Mail en Rust

Desde la v0.2 el backend es un único binario **`rvmail`** (crates/), escrito en
Rust. La web (PWA en `apps/web`) sigue en React y la sirve el mismo binario.

```
rvmail api                 API REST + tiempo real (SSE) + MCP + web estática
rvmail worker              sincronizador IMAP (IDLE, leases en Redis)
rvmail migrate             aplica migraciones y termina
rvmail healthcheck api|worker
```

## Medidas (misma máquina, misma base de datos, una cuenta)

| | Node.js (v0.1) | Rust (v0.2) | Mejora |
|---|---|---|---|
| RAM API en reposo | 110 MB | 11 MB | ×10 |
| RAM API tras 400 peticiones | 131 MB | 12 MB | ×11 |
| RAM worker (1 cuenta, IDLE) | 75–85 MB | 7 MB | ×11 |
| 400 peticiones (listar + carpetas) | 7,0 s | 5,3 s | 25 % más rápido |
| Tamaño de imagen | ~200 MB × 3 imágenes | 1 imagen distroless (~40 MB, estimado) | — |
| Binario (api + worker) | — | 11,9 MB x86_64 · ~5 MB solo worker ARM64 | — |

Los tiempos de las peticiones incluyen el arranque de `curl` en cada llamada,
así que la diferencia real de la API es mayor.

En un VPS ARM pequeño (p. ej. Oracle Ampere A1 de 1 OCPU / 6 GB, o uno de
1 GB) la app completa (api + worker) cabe en ~25 MB, dejando la memoria para
Postgres y Redis.

## Compatibilidad con la versión Node

- **Misma base de datos**: las migraciones SQL de Drizzle se copiaron 1:1 en
  `crates/rvmail/migrations`. Al arrancar, si la base viene de Node
  (`drizzle.__drizzle_migrations`), se registran como aplicadas sin reejecutarse.
- **Mismo cifrado**: `MASTER_KEY` y el formato `v1:iv:tag:datos` (AES-256-GCM)
  son idénticos; hay una prueba que descifra un secreto generado por Node.
- **Mismas sesiones y contraseñas**: JWT HS256 en la cookie `rv_session` y
  hashes argon2id (los usuarios existentes entran sin cambiar nada).
- **Mismos endpoints y JSON**, incluidos los errores (`{message, error, statusCode}`),
  por eso la web no cambió.
- **Mismos eventos y claves de Redis** (`rvmail:events:*`, leases, control).

## Arquitecturas

El Dockerfile compila de forma cruzada (sin QEMU) para `linux/amd64` y
`linux/arm64`. En local:

```bash
rustup target add aarch64-unknown-linux-gnu
sudo apt install gcc-aarch64-linux-gnu
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build --release --target aarch64-unknown-linux-gnu -p rvmail
```

## Desarrollo

```bash
make dev-up      # postgres, redis, GreenMail
make dev         # rvmail api + rvmail worker + Vite (http://localhost:5173)
cargo test && cargo clippy --all-targets -- -D warnings
```

Cambios de esquema: nuevo archivo `crates/rvmail/migrations/<AAAAMMDDhhmmss>_<nombre>.sql`
(`sqlx migrate add -r` o a mano). Nunca editar uno ya aplicado.

## Cliente web en Rust (fase C3, en curso)

`apps/web-rs` es el cliente web reescrito con **Leptos** (WebAssembly). Usa la
misma API que la PWA React y los tipos de `crates/rvmail-types`, que también usa
el servidor: si cambia un campo, el compilador avisa en los dos lados.

- Se sirve en **`/next/`** junto a la PWA (misma sesión). Cuando tenga todas las
  pantallas, pasará a `/` y se retirará la PWA React.
- Hecho: inicio de sesión, bandeja unificada y por carpeta, búsqueda, paginación,
  lectura segura (HTML saneado con `ammonia` en `<iframe sandbox>`, imágenes
  remotas bloqueadas salvo permiso), destacar, tiempo real (SSE) y diseño móvil.
- Pendiente: redactar/responder, cuentas, configuración (tokens de IA, servidor de
  correo, SSO) y los avisos de la PWA.
- Tamaño: ~1 MB de WASM (430 KB comprimido), sobre todo por el saneador HTML.

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
pnpm build:rs          # compila y copia a apps/web/dist/next (la API lo sirve)
cd apps/web-rs && cargo test && cargo clippy --target wasm32-unknown-unknown
```
