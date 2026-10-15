#!/usr/bin/env bash
# Prepara el entorno de desarrollo en tu máquina (Linux, macOS o WSL):
# dependencias, .env.dev con claves, infraestructura Docker y migraciones.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

command -v node >/dev/null || { echo "Instala Node.js 22 o superior" >&2; exit 1; }
command -v docker >/dev/null || { echo "Instala Docker Desktop / Docker Engine" >&2; exit 1; }
command -v cargo >/dev/null || { echo "Instala Rust: https://rustup.rs" >&2; exit 1; }
corepack enable >/dev/null 2>&1 || true

if [[ ! -f .env.dev ]]; then
  sed -e "s|JWT_SECRET=__GENERATE__|JWT_SECRET=$(openssl rand -hex 32)|" \
      -e "s|MASTER_KEY=__GENERATE__|MASTER_KEY=$(openssl rand -base64 32)|" \
      .env.dev.example >.env.dev
  echo "✔ .env.dev creado"
fi

pnpm install
docker compose -f docker-compose.dev.yml up -d --wait
pnpm db:migrate

cat <<'EOF'

✔ Entorno listo. Ahora:
    make dev
  y abre http://localhost:5173

  Cuenta de correo de prueba para agregar en la app:
    correo/usuario: test1@rvmail.local   contraseña: test1
    IMAP: localhost 3143 (sin SSL)       SMTP: localhost 3025 (sin SSL)
EOF
