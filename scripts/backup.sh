#!/usr/bin/env bash
# Respaldo de RV Mail: base de datos (pg_dump comprimido) + copia del .env.
# Uso: ./scripts/backup.sh [carpeta_destino]   (por defecto ./backups)
# Cron diario a las 2:30 a. m.:
#   30 2 * * * cd /opt/rvmail && ./scripts/backup.sh >> backups/backup.log 2>&1
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
if command -v rvmail >/dev/null 2>&1; then exec rvmail backup --dir "$PWD" ${1:+--dest "$1"}; fi

DEST="${1:-./backups}"
KEEP_DAYS="${KEEP_DAYS:-14}"
STAMP="$(date +%Y%m%d-%H%M%S)"
mkdir -p "$DEST"
chmod 700 "$DEST"

DB_USER="$(grep -E '^POSTGRES_USER=' .env | cut -d= -f2- || true)"
DB_NAME="$(grep -E '^POSTGRES_DB=' .env | cut -d= -f2- || true)"
DB_USER="${DB_USER:-rvmail}"
DB_NAME="${DB_NAME:-rvmail}"

docker compose exec -T postgres pg_dump -U "$DB_USER" -d "$DB_NAME" --format=custom --compress=6 >"$DEST/rvmail-$STAMP.dump"
cp .env "$DEST/env-$STAMP.bak"
chmod 600 "$DEST/env-$STAMP.bak" "$DEST/rvmail-$STAMP.dump"

find "$DEST" -name 'rvmail-*.dump' -mtime +"$KEEP_DAYS" -delete
find "$DEST" -name 'env-*.bak' -mtime +"$KEEP_DAYS" -delete

echo "$(date -Is) backup OK: $DEST/rvmail-$STAMP.dump ($(du -h "$DEST/rvmail-$STAMP.dump" | cut -f1))"
