# RV Mail

Cliente de correo **multi-cuenta**, **autoalojable** y **accesible desde cualquier
dispositivo** — de [Soluciones RV](https://solucionesrv.net).

- Bandeja unificada de todas tus cuentas (IMAP/SMTP: servidor propio, cPanel, Gmail, Outlook, Yahoo, iCloud…)
- Notificación inmediata de correo nuevo (IMAP IDLE + tiempo real en el navegador)
- Búsqueda en español sobre todas las cuentas
- App instalable (PWA) en iPhone, Android, Windows y Linux
- Contraseñas de cuentas cifradas (AES-256-GCM)
- Instalación en un comando con Docker; convive con aaPanel

> Estado: **v0.1 — base funcional.** Ver el plan completo en [docs/ROADMAP.md](docs/ROADMAP.md)
> (calendario, contactos, OAuth, PGP, IA…).

## Instalar en un VPS

```bash
git clone <URL-de-tu-repo> /opt/rvmail && cd /opt/rvmail
sudo ./scripts/install.sh --domain correo.tudominio.com
```

Detalles, aaPanel y respaldos: [docs/DEPLOY.md](docs/DEPLOY.md).

## Desarrollar

Requisitos: Node 22+, pnpm (vía `corepack enable`), Docker.

```bash
make dev-setup   # dependencias, .env.dev, postgres/redis/GreenMail, migraciones
pnpm dev         # http://localhost:5173
```

Cuenta de prueba: `test1@rvmail.local` / `test1`, IMAP `localhost:3143`, SMTP `localhost:3025`, sin SSL.

Arquitectura: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) · Guía para Claude Code: [CLAUDE.md](CLAUDE.md)

## Estructura

```
apps/api        API NestJS (auth, cuentas, mensajes, envío, tiempo real)
apps/worker     Sincronizador IMAP multi-cuenta
apps/web        PWA React + Tailwind
packages/db     Esquema y migraciones (Drizzle + PostgreSQL)
packages/shared Cifrado y eventos compartidos
deploy/         Caddy, nginx, aaPanel, cloud-init
scripts/        Instalar, actualizar, respaldar, restaurar
```
