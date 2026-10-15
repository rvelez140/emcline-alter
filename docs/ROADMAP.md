# Roadmap de RV Mail

Marca `[x]` cuando una tarea esté terminada, compilando y probada contra
GreenMail en desarrollo. Trabaja en orden: R → S → J → I → H → C, luego 1–6.

## Fase 0 — Base (incluida en el esqueleto inicial)
- [x] Monorepo pnpm, TypeScript, estructura de apps y paquetes
- [x] Docker Compose de producción y desarrollo, instalador para VPS
- [x] Esquema de BD y migración inicial
- [x] API: salud, registro/login, CRUD de cuentas con prueba de conexión
- [x] Worker: leases, sincronización inicial de encabezados, IDLE en INBOX
- [x] Web: login, alta de cuentas, bandeja unificada, lectura saneada, SSE

## Fase 0.5 — Servidor de correo, agentes de IA e infraestructura
- [x] `.env.example`, `.env.dev.example`, `.gitignore`, `.dockerignore` (el instalador no funcionaba sin ellos)
- [x] Healthcheck del worker (latido) y `/api/health` sin detalles internos
- [x] CI en GitHub Actions + imágenes multi-arquitectura (amd64/arm64) en GHCR
- [x] Integración con el Mail Server de aaPanel: dominios, buzones, verificación DNS
- [x] Inicio de sesión con las credenciales del buzón (alta automática de usuario y cuenta)
- [x] Autoconfiguración de cuentas de dominios alojados
- [x] Tokens de API con permisos (leer/borrador/enviar/organizar), por cuenta y con caducidad
- [x] Servidor MCP (`/api/mcp`) para agentes de IA + API REST con token
- [x] Instalador: detecta aaPanel, su Mail Server, el puerto del panel y el hostname de Postfix
- [ ] Verificar los nombres de función del plugin `mail_sys` contra un aaPanel real (ver docs/MAILSERVER.md)

## Fase R — Migración a Rust (por fases, sin detener el producto)

Objetivo: backend en un único binario `rvmail` (api + worker + instalador),
pensado para VPS pequeños y ARM (Oracle Ampere A1). La base de datos y el
contrato REST/SSE/MCP no cambian: cada pieza se reemplaza sin tocar las demás.

- [ ] R1. Workspace Cargo (`crates/`): `rvmail-core` (config, BD con sqlx, cifrado compatible con el formato `v1:` actual, eventos Redis)
- [ ] R2. Worker en Rust (`rvmail worker`): leases en Redis, sincronización IMAP, IDLE, control por Redis; reemplaza `apps/worker`
- [ ] R3. API en Rust (`rvmail api`, axum): auth/cookies, cuentas, correo, SSE, tokens, MCP, servidor de correo; mismos endpoints
- [ ] R4. Migraciones con `sqlx migrate` (se conservan las SQL ya aplicadas) y retiro de NestJS/Drizzle
- [ ] R5. Imagen única `rvmail` (distroless/scratch, amd64+arm64) que sirve también la web estática; compose simplificado
- [ ] R6. Comparativa de consumo (RAM/CPU/imagen) Node vs Rust documentada

## Fase S — Inicio de sesión y cuentas con Google, Microsoft y Yahoo
- [ ] S1. SSO (OpenID Connect) para entrar a RV Mail: Google, Microsoft (Entra ID), Yahoo; botones en el login; vinculación por correo verificado
- [ ] S2. Cuentas de correo por OAuth2 (XOAUTH2 en IMAP/SMTP) con refresco automático de tokens: Gmail, Outlook/M365, Yahoo
- [ ] S3. Configuración de proveedores desde el instalador/panel (client id/secret cifrados), guía de registro de apps

## Fase J — JMAP (el protocolo moderno de correo)
- [ ] J1. Cliente JMAP (RFC 8620/8621) en el worker: sesión, Mailbox/Email/changes, push (EventSource) — cuentas `protocol = jmap`
- [ ] J2. Envío por JMAP (EmailSubmission) y autodetección (`/.well-known/jmap`)
- [ ] J3. Proveedor de servidor **Stalwart** (Rust, JMAP/IMAP/SMTP/CalDAV/CardDAV) junto a aaPanel: dominios, buzones, DKIM vía su API

## Fase I — Instalador y administración (`rvmail` CLI)
- [ ] I1. `rvmail install`: asistente interactivo (dominio, proxy caddy/external, aaPanel/Stalwart/ninguno, SSO, rol primario/respaldo), autodetección de arquitectura, puertos, aaPanel, IP pública y DNS
- [ ] I2. `rvmail config`: menú para modificar cualquier ajuste después de instalar (reescribe `.env` y reinicia lo necesario)
- [ ] I3. `rvmail doctor` (DNS, TLS, puertos, conexión a panel/IMAP/SMTP), `rvmail update`, `rvmail backup`/`restore`
- [ ] I4. Instalador de una línea (`curl … | sh`) que descarga el binario correcto (x86_64/aarch64)
- [ ] I5. Soporte Podman (Quadlet + `podman compose`) además de Docker

## Fase H — Alta disponibilidad: servidor principal + respaldo ARM (Oracle)
- [ ] H1. Rol `primary`/`standby` en la instalación; red privada entre VPS con WireGuard
- [ ] H2. Réplica de Postgres en streaming hacia el respaldo (solo lectura) + backups cifrados (restic) a Oracle Object Storage/S3
- [ ] H3. MX secundario en el respaldo (Postfix en cola) para no perder correo si cae el principal; réplica de buzones (dsync) opcional
- [ ] H4. `rvmail failover promote` y guía de conmutación (DNS / Cloudflare health checks); `rvmail doctor` revisa la réplica
- [ ] H5. Perfil de recursos para VPS pequeños (límites de memoria, Postgres/Redis afinados, `WORKER_MAX_ACCOUNTS`)

## Fase C — Clientes nativos y cliente en Rust
- [ ] C1. API para apps nativas: tokens de sesión con refresh, CORS controlado, `/.well-known/rvmail` (descubrimiento del servidor)
- [ ] C2. App de escritorio y móvil con **Tauri 2** (Rust): Windows, macOS, Linux, Android, iOS; bandeja del sistema, `mailto:`, notificaciones nativas
- [ ] C3. Cliente web reescrito en Rust/WASM (Leptos) por pantallas, compartiendo tipos con el backend; la PWA React se retira al completar la paridad

## Fase 1 — Correo completo (v1)
- [ ] Sincronización incremental real con CONDSTORE/QRESYNC y detección de borrados (EXPUNGE / VANISHED)
- [ ] Redactar: editor enriquecido (TipTap), responder, responder a todos, reenviar, CC/CCO
- [ ] Adjuntos: subir al redactar, descargar y previsualizar (PDF/imágenes)
- [ ] Firmas por cuenta (HTML)
- [ ] Mover, archivar, borrar, marcar spam; selección múltiple; arrastrar y soltar
- [ ] Vista por conversaciones (hilos por `References`/`In-Reply-To`)
- [ ] Búsqueda avanzada (de:, para:, asunto:, tiene:adjunto, fechas) sobre FTS
- [ ] Autodescubrimiento de servidores (autoconfig de Thunderbird, SRV, MX conocidos)
- [ ] Notificaciones Web Push (VAPID) por cuenta, con silenciado
- [ ] Atajos de teclado estilo Gmail/eM Client
- [ ] Modo oscuro y tema por cuenta
- [ ] Pruebas e2e (Playwright) contra GreenMail

## Fase 2 — Productividad
- [ ] Enviar más tarde (cola en Redis)
- [ ] Posponer (snooze) mensajes
- [ ] Plantillas y respuestas rápidas
- [ ] Reglas/filtros del lado cliente
- [ ] Confirmación de lectura opcional, deshacer envío (ventana de N segundos)
- [ ] Exportar/importar (EML, MBOX)

## Fase 3 — Calendario, contactos, tareas, notas
- [ ] CalDAV/CardDAV (Radicale o Stalwart) + cliente en la API
- [ ] Calendario: vistas día/semana/mes, invitaciones iCal desde correos (aceptar/rechazar)
- [ ] Contactos: libreta por cuenta + autocompletado al redactar, grupos
- [ ] Tareas (VTODO) y notas
- [ ] Sincronizar calendarios/contactos de Google y Microsoft (OAuth)

## Fase 4 — Cuentas OAuth y proveedores
- [ ] (Movido a las fases S y J)
- [ ] Exchange on-premise vía EWS (opcional, evaluar demanda)

## Fase 5 — Seguridad y empresa
- [ ] 2FA (TOTP) para usuarios de la app
- [ ] PGP y S/MIME: firmar, cifrar, verificar
- [ ] Panel de administración: usuarios, cuotas, estado de workers, logs
- [ ] Multi-organización (varios clientes de Soluciones RV en una instalación) con marca blanca
- [ ] (Movido a H2) Backups programados de Postgres a S3/rclone

## Fase 6 — IA y extras
- [ ] Resumir correos e hilos, redactar respuestas, traducir (proveedor LLM configurable)
- [ ] Chat (XMPP/Matrix) — evaluar
- [ ] (Movido a C2) App de escritorio con Tauri
