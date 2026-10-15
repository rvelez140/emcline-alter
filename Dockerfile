# syntax=docker/dockerfile:1.7
# Imagen única multi-target: api, worker y web.
# El worker es Rust: se compila de forma CRUZADA (sin emulación QEMU) para
# amd64 y arm64 (Oracle Ampere A1, Raspberry Pi 4/5, Graviton…).
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
COPY apps/web/package.json apps/web/
COPY packages/db/package.json packages/db/
COPY packages/shared/package.json packages/shared/
RUN --mount=type=cache,id=pnpm,target=/pnpm/store pnpm install --frozen-lockfile
COPY . .
ARG APP_VERSION=dev
RUN pnpm -r run build
RUN pnpm --filter @rvmail/api deploy --prod /out/api

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

# ---------------------------------------------------------------- rust (compilación cruzada)
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS rust-build
ARG TARGETARCH
RUN case "$TARGETARCH" in \
      arm64) echo aarch64-unknown-linux-gnu >/rust-target; \
             apt-get update && apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
             && rm -rf /var/lib/apt/lists/* ;; \
      amd64) echo x86_64-unknown-linux-gnu >/rust-target; \
             if [ "$(uname -m)" != x86_64 ]; then apt-get update && apt-get install -y --no-install-recommends gcc-x86-64-linux-gnu libc6-dev-amd64-cross && rm -rf /var/lib/apt/lists/*; fi ;; \
      *) echo "arquitectura no soportada: $TARGETARCH" && exit 1 ;; \
    esac \
 && rustup target add "$(cat /rust-target)"
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry \
    --mount=type=cache,target=/src/target,id=cargo-target-$TARGETARCH \
    T="$(cat /rust-target)"; \
    if [ "$TARGETARCH" = amd64 ] && [ "$(uname -m)" = x86_64 ]; then unset CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER CC_x86_64_unknown_linux_gnu; fi; \
    cargo build --release --locked --target "$T" -p rvmail \
 && cp "target/$T/release/rvmail" /rvmail

# ---------------------------------------------------------------- worker (Rust)
# distroless/cc: glibc + certificados, sin shell ni gestor de paquetes (~25 MB)
FROM gcr.io/distroless/cc-debian12:nonroot AS worker
COPY --from=rust-build /rvmail /usr/local/bin/rvmail
ENV WORKER_HEARTBEAT_FILE=/tmp/rvmail-worker.heartbeat
# El worker escribe un latido tras cada supervisión (cada 30 s); sin latido en 90 s = colgado
HEALTHCHECK --interval=30s --timeout=5s --start-period=60s --retries=3 \
  CMD ["/usr/local/bin/rvmail", "healthcheck", "worker"]
ENTRYPOINT ["/usr/local/bin/rvmail"]
CMD ["worker"]

# ---------------------------------------------------------------- web
FROM nginx:1.27-alpine AS web
COPY deploy/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /repo/apps/web/dist /usr/share/nginx/html
EXPOSE 80
HEALTHCHECK --interval=15s --timeout=3s --retries=5 CMD wget -qO- http://127.0.0.1/healthz || exit 1
