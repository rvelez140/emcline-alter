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

```
             ┌──────────────┐
 Internet ──▶│ caddy (opc.) │  TLS automático (modo caddy)
             └──────┬───────┘   o aaPanel/nginx (modo external)
                    ▼
             ┌──────────────┐
             │ web (nginx)  │  PWA estática + proxy /api → api:3000
             └──────┬───────┘  (SSE sin buffering)
                    ▼
             ┌──────────────┐        ┌──────────────┐
             │ api (NestJS) │◀──────▶│ redis        │ pub/sub eventos, caché cuerpos,
             └──────┬───────┘        └──────▲───────┘ leases de cuentas
                    │                       │
                    ▼                       │
             ┌──────────────┐        ┌──────┴───────┐
             │ postgres     │◀──────▶│ worker (xN)  │ IMAP IDLE por cuenta
             └──────────────┘        └──────┬───────┘
                                            ▼
                               Servidores IMAP/SMTP de cada cuenta
                               (Stalwart propio, Gmail, M365, cPanel…)
```

### api
- Auth de usuarios de la app: contraseña con argon2id, JWT en cookie httpOnly
  (`SameSite=Lax`, `Secure` en producción). Primer usuario registrado = admin;
  después el registro queda cerrado salvo `ALLOW_SIGNUP=true`.
- CRUD de cuentas de correo, con prueba de conexión IMAP+SMTP antes de guardar.
- Lectura de mensajes desde Postgres (encabezados, snippet, flags).
- Cuerpo completo bajo demanda: conexión IMAP corta → mailparser → caché en
  Redis 1 h.
- Envío: nodemailer con el SMTP de la cuenta + `APPEND` a Enviados.
- Acciones (leído, destacado, mover, borrar): se aplican en IMAP y en BD.
- `GET /api/events` (SSE): reenvía al navegador los eventos que publica el
  worker en Redis, filtrados por usuario.

### worker
- Cada 30 s revisa cuentas activas e intenta tomar el **lease** de cada una
  (`SET key NX PX`). Si lo obtiene, abre sesión y la mantiene renovando el lease.
- Por cuenta: lista carpetas, sincroniza encabezados de los últimos
  `SYNC_DAYS` días, luego IDLE en INBOX; cada `SYNC_INTERVAL` repasa el resto
  de carpetas con UID > último conocido.
- Errores de autenticación → marca la cuenta `auth_error` y deja de reintentar
  hasta que el usuario corrija la contraseña. Errores de red → backoff
  exponencial (máx. 5 min).
- Publica `message.new`, `message.updated`, `account.status` en el canal
  Redis `rvmail:events:<userId>`.

### web (PWA)
- Vite + React 19 + Tailwind 4 + vite-plugin-pwa (service worker, manifest).
- Diseño de 3 paneles en escritorio (cuentas/carpetas · lista · lectura) y
  navegación por pantallas en móvil.
- Bandeja unificada por defecto; color por cuenta.
- HTML de mensajes saneado (DOMPurify) dentro de `<iframe sandbox>`.

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
