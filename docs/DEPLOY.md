# Despliegue de RV Mail

## Requisitos
- VPS Linux x86_64 o ARM64 (Ubuntu 22.04/24.04, Debian 12, Oracle Linux…).
  1 GB de RAM basta con el perfil "pequeño"; 2 GB o más recomendado.
- Un subdominio apuntando a la IP del VPS (registro A), p. ej. `correo.solucionesrv.net`.
- Docker (el instalador lo instala si falta) o Podman.

## Instalación recomendada: una línea

```bash
curl -fsSL https://raw.githubusercontent.com/rvelez140/emcline-alter/main/scripts/get.sh | sudo sh
```

Descarga el binario `rvmail` de tu arquitectura (estático, verificado con
SHA-256) y abre el asistente `rvmail install`, que **detecta solo**:

- arquitectura, CPU y memoria → perfil de recursos (pequeño / estándar / grande);
- Docker o Podman (`podman compose`, o Quadlet + systemd con `--engine quadlet`);
- si 80/443 están ocupados (aaPanel/nginx) → modo `external`, si no `caddy`;
- aaPanel y su Mail Server (puerto del panel, `myhostname` de Postfix);
- la IP pública y si el dominio ya apunta a este servidor.

Pregunta el dominio, el servidor de correo (aaPanel, Stalwart o ninguno), el
inicio de sesión con Google/Microsoft/Yahoo y el rol (principal o respaldo).
Genera los secretos, escribe `/opt/rvmail/.env`, descarga las imágenes,
arranca todo y, con Stalwart, hace su **configuración inicial automática**.

Sin preguntas (p. ej. desde cloud-init):

```bash
curl -fsSL …/get.sh | sudo sh -s -- --domain correo.solucionesrv.net \
  --mailserver stalwart --mail-host mail.solucionesrv.net --yes
```

`rvmail install --help` lista todas las opciones.

## Administración con `rvmail`

| Tarea | Comando |
|---|---|
| Cambiar cualquier ajuste (menú) | `sudo rvmail config` |
| Dominio / servidor de correo / SSO / recursos | `rvmail config domain` · `mailserver` · `sso` · `resources` |
| Ver o cambiar una variable | `rvmail config show` · `rvmail config set SYNC_DAYS=60` |
| Diagnóstico completo | `rvmail doctor` (DNS, HTTPS, puertos, MX/SPF/DMARC/PTR, salida por el 25, respaldos) |
| Actualizar (binario + imágenes, con respaldo previo) | `sudo rvmail update` |
| Respaldo | `sudo rvmail backup` (`--with-mail` incluye los buzones de Stalwart) |
| Restaurar | `sudo rvmail restore backups/rvmail-AAAAMMDD-HHMMSS.dump` (`--mail`: buzones de Stalwart desde restic) |
| Respaldos fuera del servidor y cron | `sudo rvmail config backup` |
| Réplica y conmutación | `rvmail ha init` · `ha join` · `ha status` · `failover promote` ([HA.md](HA.md)) |

`rvmail config` guarda el `.env` (permisos 600) y aplica el cambio: compose
recrea solo los contenedores afectados. Usa `--no-apply` para solo guardar.
Si el `.env` no está en `/opt/rvmail`, pasa `--dir` o ejecuta desde esa carpeta.

## Servidor de respaldo (alta disponibilidad)

Principal + respaldo (p. ej. un VPS ARM de Oracle) con réplica de Postgres en
streaming por WireGuard, respaldos cifrados en Oracle Object Storage y
`rvmail failover promote`: [HA.md](HA.md).

## Podman

- Con `podman compose` (o `podman-compose`) funciona igual que Docker:
  `rvmail install --engine podman`.
- Sin compose, **Quadlet** (Podman 4.4+; `host-gateway` requiere 5.3+):
  `rvmail install --engine quadlet` genera las unidades en
  `/etc/containers/systemd/rvmail-*.container` y `rvmail.target`, con un
  entorno ya resuelto en `/opt/rvmail/quadlet/rvmail.env` (600). Se gestionan
  con `systemctl start|stop|status rvmail.target` y `journalctl -u rvmail-app`.
  Requiere imágenes precompiladas (`IMAGE_PREFIX=ghcr.io/…`).

## Instalación desde el repositorio (alternativa)

### Opción A — VPS que ya tiene aaPanel (o nginx) en 80/443

```bash
cd /opt
git clone <URL-de-tu-repo> rvmail
cd rvmail
sudo ./scripts/install.sh --domain correo.solucionesrv.net
```

El instalador detecta que 80/443 están ocupados y usa el modo **external**:
la app queda escuchando en `127.0.0.1:8088` (no expuesta a Internet).
Después, en aaPanel:

1. **Website → Add site** → `correo.solucionesrv.net` (PHP: estático).
2. **SSL → Let's Encrypt** → emitir certificado.
3. **Config** del sitio → pegar el contenido de `deploy/nginx-aapanel.conf.example`
   dentro del bloque `server { }`, en lugar del `location /` existente.
4. Abrir `https://correo.solucionesrv.net` y crear el administrador.

> Si el puerto 8088 está ocupado por otro contenedor: `--port 8089`.

### Opción B — VPS limpio

```bash
sudo ./scripts/install.sh --domain correo.solucionesrv.net --email tu@correo.com
```

Usa el modo **caddy**: Caddy ocupa 80/443 y renueva el certificado solo.

### Opción C — VPS nuevo totalmente automático (cloud-init)

Edita `deploy/cloud-init.yaml` (repo, dominio, correo y tu clave SSH) y pégalo
como *user data* al crear el VPS. Deja firewall (ufw), fail2ban, Docker,
RV Mail y el respaldo diario configurados.

## Servidor de correo de aaPanel en el mismo VPS

Si el VPS tiene el Mail Server de aaPanel, el instalador lo detecta y RV Mail
administra sus dominios y buzones. Pasos de aaPanel (API y lista blanca):
[MAILSERVER.md](MAILSERVER.md). Agentes de IA: [AI-AGENT.md](AI-AGENT.md).

## Operación diaria (instalación desde el repositorio)

| Tarea | Comando |
|---|---|
| Ver estado | `make ps` |
| Logs | `make logs` |
| Actualizar a la última versión | `make update` (hace respaldo antes) |
| Respaldo manual | `make backup` → `backups/` |
| Restaurar | `./scripts/restore.sh backups/rvmail-AAAAMMDD-HHMMSS.dump` |
| Más sincronizadores | `make scale-workers N=3` |

Si el binario `rvmail` está instalado, los scripts de `scripts/` le pasan el
trabajo a él.

Respaldo diario por cron:
```
30 2 * * * /usr/local/bin/rvmail backup --dir /opt/rvmail >> /opt/rvmail/backups/backup.log 2>&1
```
Copia `backups/` fuera del servidor (rclone a Google Drive, por ejemplo).

## Importante: MASTER_KEY

Las contraseñas de las cuentas de correo se guardan cifradas con `MASTER_KEY`
(en `.env`). Si restauras la base de datos en otro servidor, usa el mismo `.env`.
Sin esa clave, la app funciona pero habrá que volver a escribir la contraseña
de cada cuenta.

## Instalar la app en los dispositivos (PWA)

- **iPhone/iPad (Safari):** abrir la URL → Compartir → *Añadir a pantalla de inicio*.
- **Android (Chrome):** menú ⋮ → *Instalar aplicación*.
- **Windows (Edge/Chrome):** icono de instalar en la barra de direcciones.
- **Linux (Chrome/Chromium):** igual que Windows.

## Imágenes y binarios precompilados

El workflow `.github/workflows/ci.yml` publica la imagen multi-arquitectura en
GHCR en cada push a `main` y, al crear un tag `v0.3.0`, una *release* con los
binarios estáticos `rvmail-x86_64-linux` y `rvmail-aarch64-linux`, `get.sh` y
`SHA256SUMS` (los usan el instalador de una línea y `rvmail update`). En el VPS puedes entonces usar
`IMAGE_PREFIX=ghcr.io/<tu-usuario>/rvmail` y `APP_VERSION=0.2.0` en `.env`
y hacer `docker compose pull && docker compose up -d` en vez de compilar.
