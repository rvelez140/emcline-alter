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

`navegador/PWA/agente IA (MCP) → web (nginx: estáticos + /api) → api (NestJS) ↔ postgres/redis ← worker (sincronización IMAP por cuenta)`
y opcionalmente `api → API de aaPanel (Mail Server del mismo VPS)` para administrar dominios y buzones.

Detalle en `docs/ARCHITECTURE.md`. Léelo antes de cambiar límites entre servicios.

## Estructura del repo

```
apps/api        API REST + SSE (NestJS 11, CommonJS). Auth, cuentas, mensajes, envío.
crates/rvmail   Binario Rust `rvmail`: worker IMAP (IDLE, leases en Redis); luego api e instalador.
crates/rvmail-core  Núcleo Rust compartido (config, cifrado compatible, modelos, eventos).
apps/web        PWA (Vite + React 19 + Tailwind 4). Interfaz responsive de 3 paneles.
packages/db     Esquema Drizzle + migraciones SQL + cliente compartido.
packages/shared Tipos y utilidades comunes (cifrado de credenciales, eventos).
deploy/         Caddyfile, ejemplo de proxy para aaPanel/nginx, cloud-init.
scripts/        Instalador, backup, actualización, utilidades de desarrollo.
```

## Comandos

```bash
make dev-up        # levanta postgres, redis y GreenMail (IMAP/SMTP de pruebas)
pnpm install
pnpm db:migrate    # aplica migraciones a la BD de desarrollo
pnpm dev           # api :3000, web :5173 (proxy /api)
make dev-worker    # worker Rust (cargo run -p rvmail -- worker)
cargo test && cargo clippy --all-targets   # Rust
pnpm build         # compila todo
pnpm typecheck
make prod-config   # valida docker-compose de producción
```

Servidor de correo integrado en desarrollo: `node scripts/fake-aapanel.mjs` y
`MAILSERVER_PROVIDER=aapanel` en `.env.dev`. Guías: `docs/MAILSERVER.md`, `docs/AI-AGENT.md`.

Cuentas de prueba en desarrollo (GreenMail las crea solas):
`test1@rvmail.local` / `test1`, `test2@rvmail.local` / `test2`,
IMAP `localhost:3143`, SMTP `localhost:3025`, sin TLS.

## Reglas del proyecto (no negociables)

1. **Multi-cuenta primero.** Toda consulta de mensajes acepta "todas las cuentas"
   (bandeja unificada) o un `accountId`. Nunca asumas una sola cuenta.
2. **Credenciales cifradas.** Contraseñas y tokens OAuth de cuentas de correo se
   guardan cifrados con AES-256-GCM (`packages/shared/src/crypto.ts`) usando
   `MASTER_KEY`. Nunca se registran en logs ni se devuelven por la API.
3. **HTML de correos = no confiable.** Se sanea con DOMPurify y se muestra en un
   `<iframe sandbox>` sin scripts. Imágenes remotas bloqueadas por defecto.
4. **El worker es el único que mantiene conexiones IMAP persistentes.** La API
   solo abre conexiones cortas bajo demanda (leer cuerpo, mover, enviar).
5. **Un worker por cuenta a la vez**, garantizado con lease en Redis
   (`crates/rvmail/src/worker/lease.rs`). Esto permite escalar `worker` con réplicas.
6. **Cambios de esquema = migración.** Edita `packages/db/src/schema.ts` y corre
   `pnpm db:generate`. Nunca edites SQL de migraciones ya aplicadas.
7. **Configuración solo por variables de entorno** (ver `.env.example`). Si
   agregas una, agrégala también a `.env.example`, a `scripts/install.sh` y a
   `docker-compose.yml`.
8. **Instalación en un comando** debe seguir funcionando. Si agregas un servicio,
   que tenga healthcheck y volumen nombrado.
9. Mensajes de error para el usuario en español, claros y sin trazas internas.
10. **Siempre cliente completo.** El servidor de correo integrado
    (`MAILSERVER_PROVIDER`, `apps/api/src/mailserver/`) es una capa extra: con
    `none` todo debe seguir funcionando. Los buzones alojados se usan como
    cuentas normales (`mail_accounts.source = 'mailserver'`).
11. **Tokens de API = mínimo privilegio.** Un endpoint acepta tokens solo si
    lleva `@RequireScopes(...)`; la lógica de correo vive en `MailService` y
    respeta `user.token` (ámbitos y cuentas). Las herramientas MCP
    (`apps/api/src/mcp/`) se registran con `defineTool` (no con `registerTool`
    directo: dispara el tiempo de compilación de tsc).

## Cómo trabajar

- Toma la siguiente tarea sin marcar de `docs/ROADMAP.md`, en orden.
- Antes de codificar una función grande, propón el plan (modo plan).
- Al terminar: `pnpm typecheck && pnpm build`, y si tocaste infraestructura,
  `make prod-config`. Marca la tarea en el roadmap.
- Commits pequeños y descriptivos en español.

## Despliegue de referencia

VPS Ubuntu con Docker y **aaPanel** ocupando 80/443. Por eso el instalador
soporta dos modos: `caddy` (Caddy gestiona TLS) y `external` (la app escucha en
`127.0.0.1:${HTTP_PORT}` y el proxy existente apunta ahí). Ver `docs/DEPLOY.md`.
