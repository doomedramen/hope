# syntax=docker/dockerfile:1

# --- Stage 1: build the web SPA -------------------------------------------
FROM node:26-slim AS web-build
WORKDIR /src
# node:26-slim no longer ships corepack; install pnpm directly instead.
RUN npm install -g pnpm@9
COPY pnpm-workspace.yaml pnpm-lock.yaml ./
COPY apps/web/package.json apps/web/
RUN cd apps/web && pnpm install --frozen-lockfile
COPY apps/web apps/web
RUN cd apps/web && pnpm build

# --- Stage 2: build the Rust workspace -------------------------------------
FROM rust:1.96-slim AS rust-build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY apps apps
COPY crates crates
COPY xtask xtask
COPY migrations migrations
RUN cargo build --release -p server

# --- Stage 3: runtime --------------------------------------------------
FROM debian:trixie-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=rust-build /src/target/release/server ./server
COPY --from=rust-build /src/migrations ./migrations
COPY --from=web-build /src/apps/web/dist ./apps/web/dist

ENV HOPE_WEB_DIST_DIR=/app/apps/web/dist
ENV HOPE_BIND_ADDR=0.0.0.0:8080
ENV HOPE_CA_CERT_PATH=/app/data/pki/ca-cert.pem
ENV HOPE_CA_KEY_PATH=/app/data/pki/ca-key.pem
ENV HOPE_SERVER_CERT_PATH=/app/data/pki/server-cert.pem
ENV HOPE_SERVER_KEY_PATH=/app/data/pki/server-key.pem
EXPOSE 8080 8443 8444

ENTRYPOINT ["/app/server"]
CMD ["serve"]
