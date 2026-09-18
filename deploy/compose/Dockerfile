# syntax=docker/dockerfile:1

# --- Stage 1: build the web SPA -------------------------------------------
FROM node:26-slim AS web-build
WORKDIR /src
RUN corepack enable
COPY pnpm-workspace.yaml package.json* ./
COPY apps/web/package.json apps/web/
RUN cd apps/web && corepack pnpm install --frozen-lockfile || corepack pnpm install
COPY apps/web apps/web
RUN cd apps/web && corepack pnpm build

# --- Stage 2: build the Rust workspace -------------------------------------
FROM rust:1.96-slim AS rust-build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY apps apps
COPY crates crates
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
EXPOSE 8080

ENTRYPOINT ["/app/server"]
CMD ["serve"]
