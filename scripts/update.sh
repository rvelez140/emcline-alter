#!/usr/bin/env bash
# Actualiza RV Mail a la última versión del repositorio.
# Hace respaldo antes, reconstruye y reinicia. Las migraciones se aplican solas.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

echo "➜ Respaldo previo…"
./scripts/backup.sh

echo "➜ Descargando cambios…"
git pull --ff-only

PREFIX="$(grep -E '^IMAGE_PREFIX=' .env | cut -d= -f2- || true)"
if [[ -z "$PREFIX" || "$PREFIX" == "rvmail" ]]; then
  echo "➜ Reconstruyendo…"
  docker compose build --pull
else
  echo "➜ Descargando imágenes nuevas…"
  docker compose pull
fi
docker compose up -d --remove-orphans

PORT="$(grep -E '^HTTP_PORT=' .env | cut -d= -f2- || echo 8088)"
for _ in $(seq 1 40); do
  if curl -fsS "http://127.0.0.1:${PORT:-8088}/api/health" >/dev/null 2>&1; then
    echo "✔ Actualizado y funcionando"
    docker image prune -f >/dev/null
    exit 0
  fi
  sleep 3
done
echo "✘ La app no responde tras actualizar. Revisa: docker compose logs api" >&2
exit 1
