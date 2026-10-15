# syntax=docker/dockerfile:1.7
# Imagen única multi-target: api, worker y web.
#   docker build --target api    -t rvmail-api .
#   docker build --target worker -t rvmail-worker .
#   docker build --target web    -t rvmail-web .

ARG NODE_VERSION=22

# ---------------------------------------------------------------- base
FROM node:${NODE_VERSION}-bookworm-slim AS base
ENV PNPM_HOME=/pnpm PATH=/pnpm:$PATH CI=true
RUN corepack enable
WORKDIR /repo

# ---------------------------------------------------------------- build
FROM base AS build
# argon2 compila binario nativo si no hay prebuild
RUN apt-get update && apt-get install -y --no-install-recommends python3 make g++ && rm -rf /var/lib/apt/lists/*
COPY pnpm-lock.yaml pnpm-workspace.yaml package.json ./
COPY apps/api/package.json apps/api/
COPY apps/worker/package.json apps/worker/
COPY apps/web/package.json apps/web/
COPY packages/db/package.json packages/db/
COPY packages/shared/package.json packages/shared/
RUN --mount=type=cache,id=pnpm,target=/pnpm/store pnpm install --frozen-lockfile
COPY . .
ARG APP_VERSION=dev
RUN pnpm -r run build
RUN pnpm --filter @rvmail/api deploy --prod /out/api \
 && pnpm --filter @rvmail/worker deploy --prod /out/worker

# ---------------------------------------------------------------- api
FROM node:${NODE_VERSION}-bookworm-slim AS api
ENV NODE_ENV=production
ARG APP_VERSION=dev
ENV APP_VERSION=${APP_VERSION}
WORKDIR /app
COPY --from=build --chown=node:node /out/api ./
USER node
EXPOSE 3000
HEALTHCHECK --interval=15s --timeout=5s --start-period=30s --retries=5 \
  CMD node -e "fetch('http://127.0.0.1:3000/api/health').then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))"
CMD ["node", "dist/main.js"]

# ---------------------------------------------------------------- worker
FROM node:${NODE_VERSION}-bookworm-slim AS worker
ENV NODE_ENV=production
WORKDIR /app
COPY --from=build --chown=node:node /out/worker ./
USER node
CMD ["node", "dist/main.js"]

# ---------------------------------------------------------------- web
FROM nginx:1.27-alpine AS web
COPY deploy/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /repo/apps/web/dist /usr/share/nginx/html
EXPOSE 80
HEALTHCHECK --interval=15s --timeout=3s --retries=5 CMD wget -qO- http://127.0.0.1/healthz || exit 1
