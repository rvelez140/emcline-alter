# Arquitectura de RV Mail

## Objetivos de diseño

| Objetivo | Decisión |
|---|---|
| Accesible desde móvil, iOS, Windows, Linux | Una sola **PWA** responsive. Se instala desde el navegador (Safari "Añadir a inicio", Chrome/Edge "Instalar app"). Notificaciones con Web Push. |
| Muchas cuentas por usuario sin degradarse | Worker dedicado con **una sesión IMAP por cuenta**, IDLE en INBOX, sincronización incremental (CONDSTORE/UID) del resto, leases en Redis para repartir cuentas entre réplicas. |
| Autoalojable y fácil de instalar | Docker Compose, instalador de un comando, secretos generados automáticamente, migraciones al arrancar. |
| Convivir con lo que ya hay en el VPS | Modo `external`: la app escucha solo en `127.0.0.1` y el proxy existente (aaPanel/nginx) publica el dominio. |
| Pocas piezas que mantener | Postgres hace de base de datos **y** de buscador (FTS en español). Redis para colas, leases, pub/sub y caché. |

## Servicios

Un solo binario Rust, `rvmail`, en una sola imagen (amd64/arm64), corre como
dos servicios: `app` (`rvmail api`) y `worker` (`rvmail worker`). Ver `docs/RUST.md`.

```
             ┌──────────────┐
 Internet ──▶│ caddy (opc.) │  TLS automático (modo caddy)
             └──────┬───────┘   o aaPanel/nginx (modo external)
                    ▼
             ┌────────────────────┐        ┌──────────────┐
 Agentes IA ▶│ app (rvmail api)   │◀──────▶│ redis        │ pub/sub eventos, caché cuerpos,
   (MCP)     │ REST·SSE·MCP·PWA   │        └──────▲───────┘ leases, límites
             └──────┬─────────┬───┘               │
                    │         └──▶ API de aaPanel (Mail Server del mismo VPS, opcional)
                    ▼                             │
             ┌──────────────┐        ┌────────────┴─────────┐
             │ postgres     │◀──────▶│ worker (rvmail, xN)  │ IMAP IDLE por cuenta
             └──────────────┘        └──────┬───────────────┘
                                            ▼
                               Servidores IMAP/SMTP de cada cuenta
                       (Mail Server propio, Gmail, M365, Yahoo, cPanel…)
```

### app (`rvmail api`, axum)
- Auth de usuarios: argon2id, JWT HS256 en cookie httpOnly (`SameSite=Lax`,
  `Secure` en producción). Primer usuario = admin; después el registro queda
  cerrado salvo `ALLOW_SIGNUP=true`. Buzones alojados entran con su contraseña;
  SSO con Google, Microsoft y Yahoo (OpenID Connect).
- Tokens de API `rvm_…` con ámbitos (`mail:read|draft|send|modify`), por cuenta
  y con caducidad; servidor MCP en `POST /api/mcp`.
- CRUD de cuentas con prueba de conexión IMAP+SMTP antes de guardar.
- Cuerpo completo bajo demanda: conexión IMAP corta → mail-parser → caché en
  Redis 1 h. Envío con lettre (SMTP de la cuenta, XOAUTH2 si es OAuth) +
  `APPEND` a Enviados.
- `GET /api/events` (SSE): una suscripción Redis por proceso, repartida en
  memoria a cada conexión del usuario.
- Sirve la PWA compilada (`WEB_DIR`) con las cabeceras de seguridad (CSP, etc.).
- Migraciones SQL embebidas (sqlx) al arrancar.

### worker (`rvmail worker`)
- Cada 30 s revisa cuentas activas e intenta tomar el **lease** de cada una
  (`SET key NX PX`); cada cuenta corre en su propia tarea tokio.
- Por cuenta: lista carpetas; por cada una pide `STATUS` (UIDNEXT, MESSAGES,
  UNSEEN y HIGHESTMODSEQ con CONDSTORE) y **salta las que no cambiaron**. En las
  que cambiaron: descarga nuevos (solo cabeceras + parte de texto para la vista
  previa, nunca adjuntos), borra los que desaparecieron y actualiza marcas.
  Después IDLE en INBOX (aviso inmediato) y repaso cada `SYNC_INTERVAL_SEC`.
- Errores de autenticación → `auth_error` sin reintentos hasta que el usuario
  corrija la contraseña. Errores de red → backoff exponencial (máx. 5 min).
- Publica `message.new`, `message.updated`, `folder.updated`, `account.status`
  en `rvmail:events:<userId>`. Healthcheck por archivo de latido.

### web (PWA)
- Vite + React 19 + Tailwind 4 + vite-plugin-pwa (service worker, manifest).
- Diseño de 3 paneles en escritorio (cuentas/carpetas · lista · lectura) y
  navegación por pantallas en móvil.
- Bandeja unificada por defecto; color por cuenta.
- HTML de mensajes saneado (DOMPurify) dentro de `<iframe sandbox>`.

### Servidor de correo integrado y agentes de IA

- `crates/rvmail/src/api/mailserver.rs`: enum `Provider` con un adaptador
  para el Mail Server de aaPanel (API firmada `request_time`/`request_token`).
  Los buzones alojados se conectan como cuentas normales y el login acepta las
  credenciales del buzón (verificadas por IMAP). Ver `docs/MAILSERVER.md`.
- `crates/rvmail/src/api/mcp.rs`: servidor MCP sin estado (Streamable HTTP, `POST /api/mcp`)
  autenticado con tokens `rvm_…` (tabla `api_tokens`, solo hash SHA-256).
  Comparte `api/mail.rs` con la API REST. Ver `docs/AI-AGENT.md`.

## Modelo de datos (resumen)

- `users` — usuarios de la app.
- `mail_accounts` — cuentas de correo de cada usuario: hosts, puertos,
  usuario, **credencial cifrada**, color, estado de sincronización.
- `folders` — carpetas por cuenta con `special_use` (\Inbox, \Sent, \Trash…),
  `uid_validity`, `last_uid`, `highest_modseq`.
- `messages` — un registro por mensaje IMAP: `uid`, `message_id`, remitente,
  destinatarios, asunto, fecha, snippet, flags, tamaño, adjuntos (sí/no),
  `search` (tsvector en español). Índice único `(folder_id, uid)`.
- `push_subscriptions` — suscripciones Web Push por dispositivo.

## Seguridad

- `MASTER_KEY` (32 bytes, base64) cifra credenciales de cuentas. Si se pierde,
  hay que volver a ingresar las contraseñas. El instalador la genera y la
  guarda en `.env` (permisos 600). **Respáldala aparte.**
- Rate limiting en login. Cabeceras de seguridad en nginx (CSP estricta).
- Ningún puerto de base de datos o Redis expuesto fuera de la red Docker.

## Escalado

- Hasta unas cuantas cientos de cuentas: 1 réplica de worker basta.
- Más: `docker compose up -d --scale worker=3`. Los leases reparten las cuentas
  sin configuración adicional.
