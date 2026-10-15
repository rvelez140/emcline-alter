# Despliegue de RV Mail

## Requisitos
- VPS Linux (Ubuntu 22.04/24.04 o Debian 12), 2 vCPU y 2 GB RAM como mínimo.
- Un subdominio apuntando a la IP del VPS (registro A), p. ej. `correo.solucionesrv.net`.
- Docker y Docker Compose v2 (el instalador los instala si faltan).

## Opción A — VPS que ya tiene aaPanel (o nginx) en 80/443

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

## Opción B — VPS limpio

```bash
sudo ./scripts/install.sh --domain correo.solucionesrv.net --email tu@correo.com
```

Usa el modo **caddy**: Caddy ocupa 80/443 y renueva el certificado solo.

## Opción C — VPS nuevo totalmente automático (cloud-init)

Edita `deploy/cloud-init.yaml` (repo, dominio, correo y tu clave SSH) y pégalo
como *user data* al crear el VPS. Deja firewall (ufw), fail2ban, Docker,
RV Mail y el respaldo diario configurados.

## Operación diaria

| Tarea | Comando |
|---|---|
| Ver estado | `make ps` |
| Logs | `make logs` |
| Actualizar a la última versión | `make update` (hace respaldo antes) |
| Respaldo manual | `make backup` → `backups/` |
| Restaurar | `./scripts/restore.sh backups/rvmail-AAAAMMDD-HHMMSS.dump` |
| Más sincronizadores | `make scale-workers N=3` |

Respaldo diario por cron:
```
30 2 * * * cd /opt/rvmail && ./scripts/backup.sh >> backups/backup.log 2>&1
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

## Imágenes precompiladas (opcional)

Si subes el repo a GitHub, el workflow `.github/workflows/ci.yml` publica
imágenes en GHCR al crear un tag `v0.1.0`. En el VPS puedes entonces usar
`IMAGE_PREFIX=ghcr.io/<tu-usuario>/rvmail` y `APP_VERSION=0.1.0` en `.env`
y hacer `docker compose pull && docker compose up -d` en vez de compilar.
