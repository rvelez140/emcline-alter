# RV Mail

Cliente de correo **multi-cuenta**, **autoalojable** y **accesible desde cualquier
dispositivo** — de [Soluciones RV](https://solucionesrv.net).

- Bandeja unificada de todas tus cuentas (IMAP/SMTP o **JMAP**: servidor propio, cPanel, Gmail, Outlook, Yahoo, iCloud, Fastmail…)
- Autodetección de la configuración de cada cuenta (JMAP, autoconfig/ISPDB, MX)
- Notificación inmediata de correo nuevo (IMAP IDLE + tiempo real en el navegador)
- Búsqueda en español sobre todas las cuentas
- App instalable (PWA) en iPhone, Android, Windows y Linux, y **app de escritorio** (Tauri 2: bandeja, avisos nativos, `mailto:`) — [docs/APPS.md](docs/APPS.md)
- Entrar con **Google, Microsoft o Yahoo** y conectar Gmail/Outlook/Yahoo sin contraseñas de aplicación ([docs/SSO.md](docs/SSO.md))
- Contraseñas y tokens de cuentas cifrados (AES-256-GCM)
- **Servidor de respaldo** (p. ej. ARM de Oracle) con réplica en streaming por WireGuard, respaldos cifrados en Oracle Object Storage y conmutación con `rvmail failover promote` ([docs/HA.md](docs/HA.md))
- **Servidor de correo integrado**: administra dominios y buzones del Mail Server de aaPanel o de **Stalwart** del mismo VPS, con verificación DNS ([docs/MAILSERVER.md](docs/MAILSERVER.md))
- **Agentes de IA**: servidor MCP para que Claude u otros agentes lean, redacten y envíen correo, con tokens de permisos limitados ([docs/AI-AGENT.md](docs/AI-AGENT.md))
- Instalación en una línea con Docker o Podman (amd64 y ARM) y administración con el CLI `rvmail` (config, doctor, update, backup); convive con aaPanel y lo detecta solo

> Estado: **v0.1 — base funcional.** Ver el plan completo en [docs/ROADMAP.md](docs/ROADMAP.md)
> (calendario, contactos, OAuth, PGP, IA…).

## Instalar en un VPS

```bash
curl -fsSL https://raw.githubusercontent.com/rvelez140/emcline-alter/main/scripts/get.sh | sudo sh
```

El asistente `rvmail install` detecta la arquitectura (x86_64/ARM), Docker o
Podman, aaPanel, los puertos y la IP pública, y lo deja todo funcionando.
Después: `rvmail config` (cambiar ajustes), `rvmail doctor` (diagnóstico),
`rvmail update`, `rvmail backup`. Detalles: [docs/DEPLOY.md](docs/DEPLOY.md).

## Desarrollar

Requisitos: Rust estable (rustup), Node 22+ con pnpm (`corepack enable`) para la web, Docker.

```bash
make dev-setup   # dependencias, .env.dev, postgres/redis/GreenMail, migraciones
make dev         # api + worker (Rust) y web: http://localhost:5173
```

Cuenta de prueba: `test1@rvmail.local` / `test1`, IMAP `localhost:3143`, SMTP `localhost:3025`, sin SSL.
Para probar el servidor de correo integrado: `node scripts/fake-aapanel.mjs` y `MAILSERVER_PROVIDER=aapanel` en `.env.dev`.

Arquitectura: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) · Guía para Claude Code: [CLAUDE.md](CLAUDE.md)

## Estructura

```
crates/rvmail       Binario Rust `rvmail`: api (REST, SSE, MCP, web), worker IMAP, migraciones
crates/rvmail-core  Núcleo Rust compartido (config, cifrado, modelos, eventos)
apps/web            PWA React + Tailwind
apps/desktop        App de escritorio/móvil (Tauri 2)
apps/web-rs         Cliente web en Rust (Leptos/WASM), en /next/ — en curso
crates/rvmail-types Tipos de la API compartidos por servidor y clientes
deploy/             Caddy, aaPanel, cloud-init
scripts/            Instalar, actualizar, respaldar, restaurar, desarrollo
```

Backend en Rust: ~25 MB de RAM para api + worker (10 veces menos que la v0.1 en
Node), una sola imagen para amd64 y ARM. Detalles: [docs/RUST.md](docs/RUST.md).
