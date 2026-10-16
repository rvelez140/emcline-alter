# Alta disponibilidad: servidor principal + respaldo (ARM de Oracle)

Escenario de referencia: un **VPS principal** potente (donde corre todo) y un
**VPS de respaldo** pequeño, por ejemplo una instancia **Ampere A1 (ARM64) de
Oracle Cloud**. Las imágenes y el binario `rvmail` son multi-arquitectura, así
que el respaldo puede ser ARM aunque el principal sea x86_64.

```
            WireGuard (UDP 51820, cifrado)
 principal  ─────────────────────────────▶  respaldo (ARM)
 10.57.0.1   Postgres: réplica en streaming   10.57.0.2
 app, worker, (Stalwart)                      solo Postgres (solo lectura)
      │                                             │
      └── restic (cifrado) ─▶ Oracle Object Storage ◀┘  (restauración)
```

## Qué se protege y cómo

| Dato | Protección | Recuperación |
|---|---|---|
| Base de datos (usuarios, cuentas, credenciales cifradas, índice de correo) | Réplica en streaming en el respaldo (segundos de retraso) + `pg_dump` diario | `rvmail failover promote` |
| `.env` (MASTER_KEY y secretos) | Copiado al respaldo al unirlo + copia diaria en `backups/` | Ya está en el respaldo |
| Buzones de **Stalwart** | `rvmail backup --with-mail` diario a restic | `rvmail failover promote --restore-mail` |
| Buzones del **Mail Server de aaPanel** | Fuera de RV Mail (en `/www/vmail`): usa el respaldo de aaPanel o restic sobre esa carpeta | Según tu respaldo de aaPanel |
| Correo de **cuentas externas** (Gmail, Outlook, cPanel…) | Vive en el proveedor | El worker vuelve a sincronizar solo |

> **¿Y el correo que llega mientras el principal está caído?** Los servidores
> que envían reintentan durante 4–5 días, así que no se pierde: se entrega en
> cuanto el MX vuelve (o apunta al respaldo promovido). Por eso no se instala
> un MX secundario: hoy se considera más un imán de spam que una protección.

## 1. Principal

Con RV Mail ya instalado:

```bash
sudo rvmail ha init
```

- instala WireGuard, crea `/etc/wireguard/rvmail0.conf` (10.57.0.1, UDP 51820)
  y lo arranca antes que Docker;
- publica Postgres **solo** en `10.57.0.1:5433` (`docker-compose.ha.yml`);
- crea el rol `replicator`, el slot `rvmail_standby` (con tope de WAL
  `PG_MAX_SLOT_WAL`, 2 GB) y la regla de `pg_hba.conf`;
- genera `/opt/rvmail/ha/standby-bundle.json` (600) con las claves de WireGuard
  del respaldo y una copia del `.env`.

**Abre el puerto UDP 51820** en el firewall del proveedor del principal.

¿Ya tienes una red privada entre los dos (VCN de Oracle, Tailscale…)? Usa
`sudo rvmail ha init --via <IP-privada-del-principal>` y no se instala WireGuard.

## 2. Respaldo (Oracle ARM)

1. Crea la instancia (Ubuntu 24.04 / Oracle Linux 9, Ampere A1). En la VCN
   no hace falta abrir nada para el respaldo: el túnel lo inicia él.
2. Instala solo el binario: `curl -fsSL …/scripts/get.sh | sudo RVMAIL_NO_INSTALL=1 sh`
3. Copia el paquete del principal (contiene secretos; nunca por correo ni chat):
   `scp root@principal:/opt/rvmail/ha/standby-bundle.json /root/`
4. `sudo rvmail ha join --bundle /root/standby-bundle.json`

`join` escribe el `.env` (rol `standby`, perfil de recursos según la RAM del
respaldo), levanta el túnel, hace la copia base con `pg_basebackup` y deja
Postgres replicando en solo lectura. El paquete se borra al terminar. En el
respaldo **solo corre Postgres**: la app y el worker arrancan al promoverlo.

## 3. Respaldos cifrados fuera del servidor (Oracle Object Storage)

En el principal (y, si quieres, también en el respaldo):

```bash
sudo rvmail config backup
```

Pide namespace, región y bucket de Object Storage y una **Customer Secret Key**
(Oracle → Perfil → Claves secretas de cliente), genera `RESTIC_PASSWORD` y
programa el respaldo diario en `/etc/cron.d/rvmail`. Cada noche:
`pg_dump` + `.env` (+ buzones de Stalwart con `--with-mail`) → `backups/` →
restic (cifrado, deduplicado) con retención `RESTIC_KEEP` (7 diarios, 4
semanales, 6 mensuales).

**Guarda `RESTIC_PASSWORD` y `MASTER_KEY` fuera de los dos servidores** (gestor
de contraseñas). Sin ellas los respaldos no sirven.

## 4. Vigilar

```bash
rvmail ha status     # principal: respaldo conectado, WAL pendiente · respaldo: retraso
rvmail doctor        # incluye la réplica, el túnel WireGuard y los respaldos
```

## 5. Si cae el principal

En el respaldo:

```bash
sudo rvmail failover promote                 # aaPanel o solo cliente
sudo rvmail failover promote --restore-mail  # con Stalwart: recupera los buzones de restic
```

1. Postgres pasa a lectura/escritura (`pg_promote`) y el rol a `primary`.
2. Arrancan app, worker (y Stalwart/Caddy según `COMPOSE_PROFILES`).
3. Cambia el DNS: A de tu dominio de RV Mail (y del servidor de correo/MX) a la
   IP del respaldo. Con Cloudflare es inmediato; con TTL bajo (300 s), minutos.
4. Publica la app en el proxy del respaldo o pasa a modo caddy:
   `rvmail config domain`.

**Evita dos principales**: antes de promover, asegúrate de que el antiguo
principal está apagado o sin red. Cuando vuelva, **no lo arranques como
principal**: mueve su `.env` y únelo como respaldo del nuevo principal
(`rvmail ha init` en el nuevo, `rvmail ha join` en el viejo).

## Recursos del respaldo

`rvmail ha join` aplica el perfil según la RAM. Una instancia A1 gratuita
(hasta 4 OCPU / 24 GB) usa el perfil "grande"; con 1 OCPU / 6 GB, el
"estándar". Mientras es respaldo solo consume Postgres (~100–300 MB).
