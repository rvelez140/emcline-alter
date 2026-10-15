# syntax=docker/dockerfile:1.7
# Imagen única `rvmail`: binario Rust (api + worker + utilidades) + web compilada.
#   docker build --target rvmail -t rvmail .
# El binario se compila de forma CRUZADA (sin emulación QEMU) para amd64 y
# arm64 (Oracle Ampere A1, Raspberry Pi 4/5, Graviton…); la web es la misma
# para todas las arquitecturas, así que se compila una sola vez.

# ---------------------------------------------------------------- web (PWA)
FROM --platform=$BUILDPLATFORM node:22-bookworm-slim AS web-build
ENV PNPM_HOME=/pnpm PATH=/pnpm:$PATH CI=true
RUN corepack enable
WORKDIR /repo
COPY pnpm-lock.yaml pnpm-workspace.yaml package.json ./
COPY apps/web/package.json apps/web/
RUN --mount=type=cache,id=pnpm,target=/pnpm/store pnpm install --frozen-lockfile --filter @rvmail/web
COPY apps/web apps/web
RUN pnpm --filter @rvmail/web build

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
# El CLI (`rvmail install`) incluye los archivos de despliegue
COPY docker-compose.yml docker-compose.ha.yml .env.example ./
COPY deploy deploy
ARG APP_VERSION=dev
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry \
    --mount=type=cache,target=/src/target,id=cargo-target-$TARGETARCH \
    T="$(cat /rust-target)"; \
    if [ "$TARGETARCH" = amd64 ] && [ "$(uname -m)" = x86_64 ]; then unset CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER CC_x86_64_unknown_linux_gnu; fi; \
    cargo build --release --locked --target "$T" -p rvmail \
 && cp "target/$T/release/rvmail" /rvmail

# ---------------------------------------------------------------- imagen final
# distroless/cc: glibc + certificados, sin shell ni gestor de paquetes
FROM gcr.io/distroless/cc-debian12:nonroot AS rvmail
ARG APP_VERSION=dev
ENV APP_VERSION=${APP_VERSION} \
    RVMAIL_ENV=production \
    WEB_DIR=/app/web \
    WORKER_HEARTBEAT_FILE=/tmp/rvmail-worker.heartbeat
COPY --from=rust-build /rvmail /usr/local/bin/rvmail
COPY --from=web-build /repo/apps/web/dist /app/web
EXPOSE 3000
# Por defecto comprueba la API; el servicio worker lo sobrescribe en compose
HEALTHCHECK --interval=15s --timeout=5s --start-period=30s --retries=5 \
  CMD ["/usr/local/bin/rvmail", "healthcheck", "api"]
ENTRYPOINT ["/usr/local/bin/rvmail"]
CMD ["api"]
