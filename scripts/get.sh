#!/bin/sh
# RV Mail — instalador de una línea. Descarga el binario `rvmail` de tu
# arquitectura (x86_64 o aarch64/ARM, estático), verifica su SHA-256 y lanza el
# asistente de instalación:
#
#   curl -fsSL https://raw.githubusercontent.com/rvelez140/emcline-alter/main/scripts/get.sh | sudo sh
#   curl -fsSL …/get.sh | sudo sh -s -- --domain correo.tudominio.com --yes
#
# Variables: RVMAIL_VERSION (por defecto latest, ej. v0.3.0), RVMAIL_REPO,
# RVMAIL_BIN_DIR (por defecto /usr/local/bin), RVMAIL_BASE_URL (espejo de los binarios),
# RVMAIL_NO_INSTALL=1 (solo descarga).
set -eu

REPO="${RVMAIL_REPO:-rvelez140/emcline-alter}"
VERSION="${RVMAIL_VERSION:-latest}"
BIN_DIR="${RVMAIL_BIN_DIR:-/usr/local/bin}"

say() { printf '\033[36m➜ %s\033[0m\n' "$*"; }
die() {
  printf '\033[31m✘ %s\033[0m\n' "$*" >&2
  exit 1
}

[ "$(uname -s)" = Linux ] || die "RV Mail se instala en Linux (Ubuntu, Debian, Oracle Linux, Rocky…)."
case "$(uname -m)" in
  x86_64 | amd64) ARCH=x86_64 ;;
  aarch64 | arm64) ARCH=aarch64 ;;
  *) die "Arquitectura no soportada: $(uname -m) (se admiten x86_64 y aarch64)" ;;
esac
[ "$(id -u)" -eq 0 ] || die "Ejecuta con sudo: curl -fsSL … | sudo sh"

if [ -n "${RVMAIL_BASE_URL:-}" ]; then
  BASE="$RVMAIL_BASE_URL" # espejo propio
elif [ "$VERSION" = latest ]; then
  BASE="https://github.com/$REPO/releases/latest/download"
else
  BASE="https://github.com/$REPO/releases/download/$VERSION"
fi

fetch() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1"
  else
    die "Hace falta curl o wget."
  fi
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM
ASSET="rvmail-$ARCH-linux"
say "Descargando $ASSET ($VERSION)…"
fetch "$BASE/$ASSET" "$TMP/rvmail" || die "No se pudo descargar $BASE/$ASSET"
fetch "$BASE/SHA256SUMS" "$TMP/SHA256SUMS" || die "No se pudo descargar SHA256SUMS"

EXPECTED="$(awk -v f="$ASSET" '$2 == f { print $1 }' "$TMP/SHA256SUMS")"
[ -n "$EXPECTED" ] || die "SHA256SUMS no incluye $ASSET"
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL="$(sha256sum "$TMP/rvmail" | awk '{ print $1 }')"
else
  ACTUAL="$(openssl dgst -sha256 "$TMP/rvmail" | awk '{ print $NF }')"
fi
[ "$EXPECTED" = "$ACTUAL" ] || die "La suma SHA-256 no coincide: descarga corrupta o manipulada"

install -m 0755 "$TMP/rvmail" "$BIN_DIR/rvmail"
say "Instalado: $("$BIN_DIR/rvmail" --version) en $BIN_DIR/rvmail"

[ "${RVMAIL_NO_INSTALL:-}" = 1 ] && exit 0
# Con `curl | sh` la entrada estándar es el script: las preguntas leen de la terminal
if (: </dev/tty) 2>/dev/null; then
  exec "$BIN_DIR/rvmail" install "$@" </dev/tty
fi
exec "$BIN_DIR/rvmail" install "$@"
