# Servidor de correo integrado (aaPanel o Stalwart)

RV Mail siempre es un **cliente completo** (bandeja unificada, búsqueda, tiempo
real, PWA, agentes de IA). Además puede administrar un servidor de correo del
mismo VPS (`MAILSERVER_PROVIDER`):

- `aapanel`: el **Mail Server de aaPanel** (Postfix + Dovecot), por su API.
- `stalwart`: **Stalwart** (servidor en Rust: SMTP, IMAP, JMAP, CalDAV/CardDAV),
  incluido en `docker-compose.yml` con el perfil `stalwart`. Ver más abajo.

En ambos casos:

- Dominios alojados: alta, baja y **verificación DNS** (A, MX, SPF, DKIM, DMARC).
- Buzones: crear, cambiar contraseña, cuota, activar/desactivar, borrar.
- **Cada buzón entra a RV Mail con su correo y contraseña**: la primera vez se
  crea su usuario y su cuenta se configura sola (sin datos de IMAP/SMTP).
- Al agregar una cuenta de un dominio alojado, el formulario se rellena solo.

Con `MAILSERVER_PROVIDER=none` todo esto se desactiva y RV Mail funciona solo
como cliente de cualquier proveedor.

## Cómo se conecta

```
contenedor api ──HTTPS──▶ host.docker.internal:<puerto del panel>   (API de aaPanel)
api / worker   ──IMAPS──▶ MAILSERVER_HOSTNAME:993  ─┐
               ──SMTPS──▶ MAILSERVER_HOSTNAME:465  ─┴─▶ resuelto a la IP del host (extra_hosts)
```

Dentro de los contenedores, `MAILSERVER_HOSTNAME` (p. ej. `mail.solucionesrv.net`)
se resuelve a la IP del propio VPS. Así el certificado TLS de Dovecot/Postfix
coincide y el tráfico no sale a internet.

## Configuración (el instalador lo hace casi todo)

`sudo rvmail install` (o `./scripts/install.sh`) detecta aaPanel y su
Mail Server, lee el puerto del panel y el `myhostname` de Postfix, y te pide la
clave de la API. A mano, en `.env`:

```ini
MAILSERVER_PROVIDER=aapanel
MAILSERVER_HOSTNAME=mail.tudominio.com
MAILSERVER_IMAP_PORT=993
MAILSERVER_SMTP_PORT=465
AAPANEL_URL=https://host.docker.internal:7800   # puerto: /www/server/panel/data/port.pl
AAPANEL_API_KEY=xxxxxxxx
AAPANEL_VERIFY_TLS=false                         # el panel usa certificado autofirmado
```

Después: `docker compose up -d`.

### En aaPanel

1. **Settings → API interface**: actívala y copia la clave en `AAPANEL_API_KEY`.
2. En la **lista blanca de IPs** de esa misma pantalla, añade la IP fija de la
   API de RV Mail: `172.30.57.10` (variable `API_IP`; la red es `DOCKER_SUBNET`).
3. Si el certificado de Dovecot es autofirmado (no de Let's Encrypt), pon
   `IMAP_ALLOW_SELF_SIGNED=true`. Lo ideal es emitir el certificado de
   `mail.tudominio.com` en aaPanel → Mail Server → SSL.
4. Si usas el firewall de aaPanel/UFW, permite desde `172.30.57.0/24` el puerto
   del panel, 993 y 465 (suelen estar abiertos ya para el correo).

Comprueba la conexión en **RV Mail → Configuración → Servidor de correo**.

## Verificación de la integración

El adaptador usa las funciones del plugin `mail_sys` de aaPanel
(`crates/rvmail/src/api/mailserver.rs`, `impl Provider`):
`get_domains`, `add_domain_new`, `delete_domain`, `get_mailboxs`, `add_mailbox`,
`update_mailbox`, `delete_mailbox`. aaPanel no publica la documentación de este
plugin; si tu versión usa otros nombres o parámetros, compruébalos en el VPS:

```bash
grep -n "def " /www/server/panel/plugin/mail_sys/mail_sys_main.py
```

y ajusta el nombre en `impl Provider`. En desarrollo, `node scripts/fake-aapanel.mjs` simula
la API (con firma real) sobre GreenMail.

## DNS de cada dominio

| Registro | Valor |
|---|---|
| A `mail.tudominio.com` | IP pública del VPS |
| MX `tudominio.com` | `10 mail.tudominio.com` |
| TXT `tudominio.com` | `v=spf1 mx a ~all` |
| TXT `default._domainkey` | clave DKIM (aaPanel → Mail Server → dominio) |
| TXT `_dmarc` | `v=DMARC1; p=quarantine; rua=mailto:postmaster@tudominio.com` |
| PTR (en tu proveedor del VPS) | `mail.tudominio.com` |

RV Mail muestra en verde/ámbar cuáles están bien al abrir cada dominio.

## Stalwart (alternativa a aaPanel, con JMAP)

[Stalwart](https://stalw.art) es un servidor de correo completo escrito en Rust.
RV Mail lo administra por su API JMAP de gestión (`urn:stalwart:jmap`,
`crates/rvmail/src/api/stalwart.rs`) y los buzones alojados se conectan por
**JMAP** (RFC 8620/8621) en vez de IMAP: sincronización por cambios y aviso
inmediato por EventSource, sin conexiones IDLE.

> **Elige uno.** Postfix de aaPanel y Stalwart usan los mismos puertos (25, 465,
> 587, 993). En un VPS con el Mail Server de aaPanel activo, sigue con
> `aapanel`; usa `stalwart` en un VPS limpio o tras desactivar el de aaPanel.

`sudo rvmail install --mailserver stalwart` (o `sudo rvmail config mailserver` en una instalación existente) lo deja configurado, incluida la configuración inicial de Stalwart. A mano, en `.env`:

```ini
MAILSERVER_PROVIDER=stalwart
MAILSERVER_HOSTNAME=mail.tudominio.com
STALWART_URL=http://stalwart:8080          # red interna de Docker
STALWART_RECOVERY_PASSWORD=<larga y aleatoria>  # solo para la configuración inicial
STALWART_ADMIN_USER=                       # los rellena rvmail con el administrador
STALWART_ADMIN_PASSWORD=                   # permanente (admin@<dominio>) tras el bootstrap
MAILSERVER_JMAP_URL=https://mail.tudominio.com   # URL JMAP que verán los clientes
COMPOSE_PROFILES=stalwart                  # (o caddy,stalwart)
```

`STALWART_RECOVERY_PASSWORD` se pasa a Stalwart como `STALWART_RECOVERY_ADMIN`.
En el primer arranque Stalwart queda en modo de configuración inicial; `rvmail`
la completa por JMAP (`x:Bootstrap/set`: nombre del servidor, dominio principal,
claves DKIM) y guarda en `.env` el administrador permanente que devuelve
Stalwart.

> **Certificado TLS del correo.** Los puertos 80/443 los usa Caddy o aaPanel,
> así que Stalwart no puede pedir su certificado por HTTP. Configúralo en su
> panel (`ssh -L 8180:127.0.0.1:8180` y abre `http://localhost:8180`) con
> ACME por **DNS-01** (Cloudflare u otro proveedor). Hasta entonces usa un
> certificado autofirmado (`IMAP_ALLOW_SELF_SIGNED=true` para RV Mail). La
administración (puerto 8080 del contenedor) solo se publica en
`127.0.0.1:${STALWART_HTTP_PORT:-8180}`; publica `MAILSERVER_JMAP_URL` en tu
proxy hacia ese puerto para que las apps externas usen JMAP.

Qué hace RV Mail con Stalwart:

- Dominios: alta, baja (borra antes sus firmas DKIM; se niega si quedan
  buzones) y **DNS esperado exacto**: lee la zona que publica Stalwart
  (MX, SPF, DKIM, DMARC…) y la compara con lo que resuelve el DNS público.
- Buzones: crear, contraseña, cuota, borrar. Las cuentas de administrador no
  se listan.
- Login con el buzón y vinculación como cuenta `protocol = jmap`.
- En **Agregar cuenta**, un correo de un dominio alojado elige JMAP solo.

## Cuentas JMAP de otros proveedores

Cualquier cuenta puede usar JMAP si su servidor lo ofrece (Fastmail, Stalwart,
Cyrus…): en **Agregar cuenta → Tipo de conexión → JMAP**. RV Mail lo detecta en
`https://<dominio>/.well-known/jmap` (también `mail.` y `jmap.`). El envío usa
`EmailSubmission` y los borradores, flags y movimientos van por `Email/set`.
