# RV Mail — atajos de desarrollo y operación
.DEFAULT_GOAL := help
DEV := docker compose -f docker-compose.dev.yml

## ---------- Desarrollo
dev-setup: ## Prepara todo el entorno de desarrollo (una vez)
	./scripts/dev-setup.sh

dev-up: ## Levanta postgres, redis y GreenMail de desarrollo
	$(DEV) up -d --wait

dev-down: ## Detiene la infraestructura de desarrollo
	$(DEV) down

dev-reset: ## Borra los datos de desarrollo y vuelve a empezar
	$(DEV) down -v
	$(DEV) up -d --wait
	pnpm db:migrate

dev: ## Corre api y web con recarga en caliente (el worker: make dev-worker)
	pnpm dev

dev-worker: ## Corre el worker Rust con las variables de .env.dev
	set -a; . ./.env.dev; set +a; cargo run -p rvmail -- worker

check: ## Typecheck + build de todo (TypeScript y Rust)
	pnpm typecheck && pnpm build
	cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test

## ---------- Producción (en el VPS)
install: ## Instalador interactivo
	sudo ./scripts/install.sh

up: ## Construye e inicia la app
	docker compose up -d --build --remove-orphans

down: ## Detiene la app (los datos se conservan)
	docker compose down

ps: ## Estado de los contenedores
	docker compose ps

logs: ## Logs en vivo de api, worker y web
	docker compose logs -f --tail=100 api worker web

update: ## Respaldo + actualización + reinicio
	./scripts/update.sh

backup: ## Respaldo de la base de datos y del .env
	./scripts/backup.sh

scale-workers: ## Escala sincronizadores: make scale-workers N=3
	docker compose up -d --scale worker=$(N)

prod-config: ## Valida el docker-compose de producción con un .env de ejemplo
	@printf 'POSTGRES_PASSWORD=x\nREDIS_PASSWORD=x\nJWT_SECRET=x\nMASTER_KEY=x\n' > .env.ci
	@docker compose --env-file .env.example --env-file .env.ci config --quiet && echo "docker-compose.yml OK"; \
	  st=$$?; rm -f .env.ci; exit $$st
	@docker compose -f docker-compose.dev.yml config --quiet && echo "docker-compose.dev.yml OK"

db-shell: ## Consola psql de producción
	docker compose exec postgres psql -U rvmail rvmail

help:
	@grep -hE '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2}'
