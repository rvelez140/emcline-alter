# Roadmap de RV Mail

Marca `[x]` cuando una tarea esté terminada, compilando y probada contra
GreenMail en desarrollo. Trabaja en orden.

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
- [ ] Soporte Podman (Quadlet) y prueba en VPS ARM
- [ ] Asistente web de instalación (dominio, DNS, proxy) en el primer arranque
- [ ] API para apps nativas: refresh tokens, CORS, `/.well-known/rvmail`; apps Capacitor (móvil) y Tauri (escritorio)

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
- [ ] Enviar más tarde (cola en Redis + BullMQ)
- [ ] Posponer (snooze) mensajes
- [ ] Plantillas y respuestas rápidas
- [ ] Reglas/filtros del lado cliente
- [ ] Confirmación de lectura opcional, deshacer envío (ventana de N segundos)
- [ ] Exportar/importar (EML, MBOX)

## Fase 3 — Calendario, contactos, tareas, notas
- [ ] Servicio Radicale (CalDAV/CardDAV) en compose + cliente CalDAV/CardDAV en la API (`tsdav`)
- [ ] Calendario: vistas día/semana/mes, invitaciones iCal desde correos (aceptar/rechazar)
- [ ] Contactos: libreta por cuenta + autocompletado al redactar, grupos
- [ ] Tareas (VTODO) y notas
- [ ] Sincronizar calendarios/contactos de Google y Microsoft (OAuth)

## Fase 4 — Cuentas OAuth y proveedores
- [ ] Gmail con OAuth2 (XOAUTH2 en IMAP/SMTP) — requiere app en Google Cloud
- [ ] Microsoft 365 / Outlook.com con OAuth2 — requiere app en Entra ID
- [ ] JMAP para servidores que lo soportan (Stalwart)
- [ ] Exchange on-premise vía EWS (opcional, evaluar demanda)

## Fase 5 — Seguridad y empresa
- [ ] 2FA (TOTP) para usuarios de la app
- [ ] PGP (openpgp.js) y S/MIME: firmar, cifrar, verificar
- [ ] Panel de administración: usuarios, cuotas, estado de workers, logs
- [ ] Multi-organización (varios clientes de Soluciones RV en una instalación) con marca blanca
- [ ] Backups programados de Postgres a S3/rclone

## Fase 6 — IA y extras
- [ ] Resumir correos e hilos, redactar respuestas, traducir (proveedor LLM configurable)
- [ ] Chat (XMPP/Matrix) — evaluar
- [ ] App de escritorio envolviendo la PWA (Tauri) si hace falta integración con el SO
