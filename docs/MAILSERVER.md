# Servidor de correo integrado (aaPanel Mail Server)

RV Mail siempre es un **cliente completo** (bandeja unificada, búsqueda, tiempo
real, PWA, agentes de IA). Además, si en el mismo VPS está el **Mail Server de
aaPanel** (Postfix + Dovecot), RV Mail lo administra:

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

`sudo ./scripts/install.sh --domain correo.tudominio.com` detecta aaPanel y su
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
(`apps/api/src/mailserver/mailserver.service.ts`, constante `FN`):
`get_domains`, `add_domain_new`, `delete_domain`, `get_mailboxs`, `add_mailbox`,
`update_mailbox`, `delete_mailbox`. aaPanel no publica la documentación de este
plugin; si tu versión usa otros nombres o parámetros, compruébalos en el VPS:

```bash
grep -n "def " /www/server/panel/plugin/mail_sys/mail_sys_main.py
```

y ajusta la constante `FN`. En desarrollo, `node scripts/fake-aapanel.mjs` simula
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
