#!/usr/bin/env bash
# RV Mail — instalador para VPS (Ubuntu 22.04/24.04, Debian 12)
#
# Uso típico (desde el repo clonado):
#   sudo ./scripts/install.sh --domain correo.solucionesrv.net
#
# Opciones:
#   --domain DOMINIO     Dominio público de la app (obligatorio)
#   --mode MODO          caddy | external | auto (por defecto auto: si 80/443
#                        están ocupados —p. ej. aaPanel— usa external)
#   --email CORREO       Correo para Let's Encrypt (modo caddy)
#   --port PUERTO        Puerto local en modo external (por defecto 8088)
#   --yes                No preguntar (instala Docker si falta, etc.)
set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOMAIN=""
MODE="auto"
ACME_EMAIL=""
HTTP_PORT="8088"
ASSUME_YES="false"

c_ok() { printf '\033[32m✔ %s\033[0m\n' "$*"; }
c_info() { printf '\033[36m➜ %s\033[0m\n' "$*"; }
c_warn() { printf '\033[33m! %s\033[0m\n' "$*"; }
die() {
  printf '\033[31m✘ %s\033[0m\n' "$*" >&2
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --domain) DOMAIN="${2:-}"; shift 2 ;;
    --mode) MODE="${2:-}"; shift 2 ;;
    --email) ACME_EMAIL="${2:-}"; shift 2 ;;
    --port) HTTP_PORT="${2:-}"; shift 2 ;;
    --yes | -y) ASSUME_YES="true"; shift ;;
    -h | --help) sed -n '2,16p' "$0"; exit 0 ;;
    *) die "Opción desconocida: $1" ;;
  esac
done

confirm() {
  [[ "$ASSUME_YES" == "true" ]] && return 0
  read -r -p "$1 [s/N] " ans
  [[ "$ans" =~ ^[sSyY]$ ]]
}

[[ $EUID -eq 0 ]] || die "Ejecuta con sudo: sudo $0 $*"
cd "$APP_DIR"
[[ -f docker-compose.yml ]] || die "No encuentro docker-compose.yml en $APP_DIR"

# ---------------------------------------------------------------- dominio
if [[ -z "$DOMAIN" ]]; then
  [[ "$ASSUME_YES" == "true" ]] && die "Falta --domain"
  read -r -p "Dominio para RV Mail (ej. correo.solucionesrv.net): " DOMAIN
fi
[[ "$DOMAIN" =~ ^[A-Za-z0-9.-]+\.[A-Za-z]{2,}$ ]] || die "Dominio no válido: $DOMAIN"

# ---------------------------------------------------------------- docker
if ! command -v docker >/dev/null 2>&1; then
  c_warn "Docker no está instalado."
  confirm "¿Instalar Docker ahora (script oficial get.docker.com)?" || die "Docker es necesario."
  curl -fsSL https://get.docker.com | sh
  systemctl enable --now docker
fi
docker compose version >/dev/null 2>&1 || die "Falta el plugin 'docker compose' (v2)."
c_ok "Docker $(docker version --format '{{.Server.Version}}' 2>/dev/null || echo '?')"

# ---------------------------------------------------------------- modo de proxy
port_busy() { ss -ltnH "( sport = :$1 )" 2>/dev/null | grep -q .; }
if [[ "$MODE" == "auto" ]]; then
  if port_busy 80 || port_busy 443; then
    MODE="external"
    c_info "Los puertos 80/443 ya están en uso (aaPanel/nginx). Modo: external"
  else
    MODE="caddy"
    c_info "Puertos 80/443 libres. Modo: caddy (TLS automático)"
  fi
fi
[[ "$MODE" == "caddy" || "$MODE" == "external" ]] || die "--mode debe ser caddy, external o auto"
if [[ "$MODE" == "external" ]] && port_busy "$HTTP_PORT"; then
  die "El puerto local $HTTP_PORT está ocupado. Usa --port con otro (ej. 8089)."
fi

# ---------------------------------------------------------------- .env
gen() { openssl rand -hex 24; }
set_env() {
  local key="$1" val="$2"
  if grep -q "^${key}=" .env; then
    sed -i "s|^${key}=.*|${key}=${val//|/\\|}|" .env
  else
    echo "${key}=${val}" >>.env
  fi
}
get_env() { grep -E "^$1=" .env | head -1 | cut -d= -f2-; }

if [[ ! -f .env ]]; then
  cp .env.example .env
  chmod 600 .env
  set_env POSTGRES_PASSWORD "$(gen)"
  set_env REDIS_PASSWORD "$(gen)"
  set_env JWT_SECRET "$(openssl rand -hex 32)"
  set_env MASTER_KEY "$(openssl rand -base64 32)"
  c_ok "Archivo .env creado con secretos nuevos"
else
  c_info "Usando .env existente (no se cambian los secretos)"
  for k in POSTGRES_PASSWORD REDIS_PASSWORD JWT_SECRET MASTER_KEY; do
    [[ -n "$(get_env "$k")" ]] || die "$k está vacío en .env"
  done
fi

set_env DOMAIN "$DOMAIN"
set_env PROXY_MODE "$MODE"
set_env HTTP_PORT "$HTTP_PORT"
[[ -n "$ACME_EMAIL" ]] && set_env ACME_EMAIL "$ACME_EMAIL"
if [[ "$MODE" == "caddy" ]]; then
  set_env COMPOSE_PROFILES "caddy"
else
  set_env COMPOSE_PROFILES ""
fi

# ---------------------------------------------------------------- arranque
c_info "Construyendo imágenes (la primera vez tarda unos minutos)…"
docker compose build --pull
c_info "Iniciando servicios…"
docker compose up -d --remove-orphans

c_info "Esperando a que la aplicación responda…"
ok="false"
for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:${HTTP_PORT}/api/health" >/dev/null 2>&1; then
    ok="true"
    break
  fi
  sleep 3
done
[[ "$ok" == "true" ]] || {
  docker compose ps
  die "La app no respondió. Revisa: docker compose logs api worker web"
}
c_ok "RV Mail está funcionando"

# ---------------------------------------------------------------- resumen
echo
echo "────────────────────────────────────────────────────────────"
echo " RV Mail instalado en $APP_DIR"
echo "────────────────────────────────────────────────────────────"
if [[ "$MODE" == "caddy" ]]; then
  echo " URL:  https://$DOMAIN"
  echo " Caddy pedirá el certificado TLS en cuanto el DNS apunte a este servidor."
else
  echo " La app escucha en 127.0.0.1:$HTTP_PORT"
  echo " Falta publicarla en tu proxy (aaPanel/nginx):"
  echo "   1. aaPanel → Website → Add site: $DOMAIN"
  echo "   2. Activa SSL (Let's Encrypt)"
  echo "   3. Pega en la config del sitio: deploy/nginx-aapanel.conf.example"
  echo "   Luego abre https://$DOMAIN"
fi
echo
echo " Primer acceso: crea la cuenta de administrador en la pantalla inicial."
echo
c_warn "RESPALDA el archivo .env (sobre todo MASTER_KEY) fuera del servidor."
echo " Backups de la base de datos:  make backup   (o programa scripts/backup.sh en cron)"
echo "────────────────────────────────────────────────────────────"
