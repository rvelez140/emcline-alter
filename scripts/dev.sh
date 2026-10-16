#!/usr/bin/env bash
# Desarrollo: API y worker Rust + web Vite con recarga en caliente.
# Requiere `make dev-up` (postgres, redis, GreenMail) y .env.dev (make dev-setup).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
set -a; . ./.env.dev; set +a
cargo build -q -p rvmail
trap 'kill 0' EXIT INT TERM
./target/debug/rvmail api &
./target/debug/rvmail worker &
pnpm --filter @rvmail/web dev &
wait
