#!/usr/bin/env bash
# Restaura un respaldo de la base de datos creado con backup.sh.
# Uso: ./scripts/restore.sh backups/rvmail-AAAAMMDD-HHMMSS.dump
# OJO: el .env debe tener la MISMA MASTER_KEY que cuando se hizo el respaldo.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
if command -v rvmail >/dev/null 2>&1; then exec rvmail restore --dir "$PWD" "$@"; fi

FILE="${1:?Indica el archivo .dump a restaurar}"
[[ -f "$FILE" ]] || { echo "No existe $FILE" >&2; exit 1; }

DB_USER="$(grep -E '^POSTGRES_USER=' .env | cut -d= -f2- || true)"
DB_NAME="$(grep -E '^POSTGRES_DB=' .env | cut -d= -f2- || true)"
DB_USER="${DB_USER:-rvmail}"
DB_NAME="${DB_NAME:-rvmail}"

read -r -p "Esto reemplaza la base de datos actual con $FILE. ¿Continuar? [s/N] " ans
[[ "$ans" =~ ^[sS]$ ]] || exit 1

docker compose stop app worker
docker compose exec -T postgres pg_restore -U "$DB_USER" -d "$DB_NAME" --clean --if-exists --no-owner <"$FILE"
docker compose start app worker
echo "Restauración completada."
