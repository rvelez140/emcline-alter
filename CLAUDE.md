# CLAUDE.md — Guía para Claude Code

Este archivo lo lee Claude Code al empezar cada sesión. Mantenlo actualizado:
cuando termines una tarea del roadmap, márcala en `docs/ROADMAP.md`.

## Qué es

**RV Mail** es un cliente de correo **web, multi-cuenta y autoalojable** de
Soluciones RV, en la línea de eM Client: correo, calendario, contactos, tareas
y notas en una sola interfaz. Se instala en un VPS con Docker en un comando y se
usa desde cualquier navegador; en móvil (iOS/Android), Windows y Linux se
instala como **PWA** (app instalable, con notificaciones push).

El producto es propio de Soluciones RV. Idioma de la interfaz: **español** por
defecto (preparado para i18n). Idioma del código y comentarios: inglés en
identificadores, español permitido en comentarios y documentación.

## Arquitectura en una línea

`navegador/PWA/agente IA (MCP) → rvmail api (Rust/axum: REST + SSE + MCP + web) ↔ postgres/redis ← rvmail worker (Rust: IMAP por cuenta)`
y opcionalmente `rvmail api → API de aaPanel (Mail Server del mismo VPS)` para administrar dominios y buzones.

Detalle en `docs/ARCHITECTURE.md` y `docs/RUST.md`. Léelos antes de cambiar límites entre servicios.

## Estructura del repo

```
crates/rvmail       Binario único `rvmail`: api (axum), worker (IMAP/IDLE), migrate, healthcheck.
  src/api/          auth, accounts, mail (MailService), events (SSE), tokens, mcp, mailserver, sso
  src/worker/       supervisor + leases en Redis + sesión IMAP por cuenta
  src/net/          IMAP (TLS/STARTTLS/XOAUTH2) y TLS (rustls/ring)
  migrations/       SQL de la base (sqlx, embebidas en el binario)
crates/rvmail-core  Núcleo compartido: config, cifrado (AES-256-GCM), modelos, eventos.
apps/web            PWA (Vite + React 19 + Tailwind 4). Interfaz responsive de 3 paneles.
deploy/             Caddyfile, ejemplo de proxy para aaPanel/nginx, cloud-init.
scripts/            Instalador, backup, actualización, desarrollo, simulador de aaPanel.
```

## Comandos

```bash
make dev-up        # levanta postgres, redis y GreenMail (IMAP/SMTP de pruebas)
pnpm install       # dependencias de la web
pnpm db:migrate    # = cargo run -p rvmail -- migrate
make dev           # rvmail api :3000 + rvmail worker + web :5173 (proxy /api)
cargo test && cargo clippy --all-targets -- -D warnings
pnpm typecheck && pnpm build   # web
make check         # todo lo anterior
make prod-config   # valida docker-compose de producción
```

CLI de administración del VPS (`crates/rvmail/src/cli/`): `rvmail install | config |
doctor | update | backup | restore`. Incluye `docker-compose.yml`, `.env.example` y
`deploy/` dentro del binario (`cli/assets.rs`): si cambias esos archivos, el
instalador de una línea los distribuye en la siguiente versión. Las variables que
solo usa el CLI empiezan por `RVMAIL_`.

Servidor de correo integrado en desarrollo: `node scripts/fake-aapanel.mjs` y
`MAILSERVER_PROVIDER=aapanel` en `.env.dev`. Guías: `docs/MAILSERVER.md`, `docs/AI-AGENT.md`.

Cuentas de prueba en desarrollo (GreenMail las crea solas):
`test1@rvmail.local` / `test1`, `test2@rvmail.local` / `test2`,
IMAP `localhost:3143`, SMTP `localhost:3025`, sin TLS.

## Reglas del proyecto (no negociables)

1. **Multi-cuenta primero.** Toda consulta de mensajes acepta "todas las cuentas"
   (bandeja unificada) o un `accountId`. Nunca asumas una sola cuenta.
2. **Credenciales cifradas.** Contraseñas y tokens OAuth de cuentas de correo se
   guardan cifrados con AES-256-GCM (`crates/rvmail-core/src/crypto.rs`) usando
   `MASTER_KEY`. Nunca se registran en logs ni se devuelven por la API.
3. **HTML de correos = no confiable.** Se sanea con DOMPurify y se muestra en un
   `<iframe sandbox>` sin scripts. Imágenes remotas bloqueadas por defecto.
4. **El worker es el único que mantiene conexiones IMAP persistentes.** La API
   solo abre conexiones cortas bajo demanda (leer cuerpo, mover, enviar).
5. **Un worker por cuenta a la vez**, garantizado con lease en Redis
   (`crates/rvmail/src/worker/lease.rs`). Esto permite escalar `worker` con réplicas.
6. **Cambios de esquema = migración.** Nuevo archivo en `crates/rvmail/migrations/`
   (`<AAAAMMDDhhmmss>_<nombre>.sql`) y ajusta `rvmail-core/src/models.rs`.
   Nunca edites SQL de migraciones ya aplicadas.
7. **Configuración solo por variables de entorno** (ver `.env.example`). Si
   agregas una, agrégala también a `.env.example`, a `scripts/install.sh` y a
   `docker-compose.yml`.
8. **Instalación en un comando** debe seguir funcionando. Si agregas un servicio,
   que tenga healthcheck y volumen nombrado.
9. Mensajes de error para el usuario en español, claros y sin trazas internas.
10. **Siempre cliente completo.** El servidor de correo integrado
    (`MAILSERVER_PROVIDER`, `crates/rvmail/src/api/mailserver.rs`) es una capa extra: con
    `none` todo debe seguir funcionando. Los buzones alojados se usan como
    cuentas normales (`mail_accounts.source = 'mailserver'`).
11. **Tokens de API = mínimo privilegio.** Extractores de axum: `Session`
    (solo navegador), `Admin` (navegador + admin) y `Authed` (navegador o token;
    el handler exige su ámbito con `user.require("mail:…")`). La lógica de correo
    vive en `api/mail.rs` y respeta `user.token` (ámbitos y cuentas). Las
    herramientas MCP (`api/mcp.rs`) se listan según los ámbitos del token.
12. **Rust primero.** Código nuevo de backend en Rust; `cargo clippy -D warnings`
    limpio. Nada de `unwrap()` en rutas de petición: errores con `ApiError`.

## Cómo trabajar

- Toma la siguiente tarea sin marcar de `docs/ROADMAP.md`, en orden.
- Antes de codificar una función grande, propón el plan (modo plan).
- Al terminar: `make check`, y si tocaste infraestructura, `make prod-config`.
  Marca la tarea en el roadmap.
- Commits pequeños y descriptivos en español.

## Despliegue de referencia

VPS Ubuntu con Docker y **aaPanel** ocupando 80/443. Por eso el instalador
soporta dos modos: `caddy` (Caddy gestiona TLS) y `external` (la app escucha en
`127.0.0.1:${HTTP_PORT}` y el proxy existente apunta ahí). Ver `docs/DEPLOY.md`.
