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
# Compile the dependency graph in a stable layer. Source changes then rebuild
# the application without recompiling every registry and path dependency.
COPY apps/server/Cargo.toml apps/server/Cargo.toml
COPY apps/agent/Cargo.toml apps/agent/Cargo.toml
COPY crates/domain/Cargo.toml crates/domain/Cargo.toml
COPY crates/jobs/Cargo.toml crates/jobs/Cargo.toml
COPY crates/protocol/Cargo.toml crates/protocol/Cargo.toml
COPY crates/release/Cargo.toml crates/release/Cargo.toml
COPY xtask/Cargo.toml xtask/Cargo.toml
RUN mkdir -p \
      apps/server/src apps/agent/src crates/domain/src crates/jobs/src \
      crates/protocol/src crates/release/src xtask/src \
    && printf 'fn main() {}\n' > apps/server/src/main.rs \
    && printf 'fn main() {}\n' > apps/agent/src/main.rs \
    && printf 'fn main() {}\n' > apps/agent/build.rs \
    && printf 'pub fn placeholder() {}\n' > crates/domain/src/lib.rs \
    && printf 'pub fn placeholder() {}\n' > crates/jobs/src/lib.rs \
    && printf 'pub fn placeholder() {}\n' > crates/protocol/src/lib.rs \
    && printf 'pub fn placeholder() {}\n' > crates/release/src/lib.rs \
    && printf 'fn main() {}\n' > xtask/src/main.rs
RUN cargo build --release -p server

COPY apps/server apps/server
COPY apps/agent apps/agent
COPY crates crates
COPY xtask xtask
COPY migrations migrations
COPY deploy/agent deploy/agent
# COPY preserves source mtimes; refresh them so Cargo notices the real sources
# replacing the placeholder files used by the dependency layer.
RUN find apps/server apps/agent crates xtask -type f -name '*.rs' -exec touch {} + \
    && cargo build --release -p server

# --- Stage 3: runtime --------------------------------------------------
FROM debian:trixie-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=rust-build /src/target/release/server ./server
COPY --from=rust-build /src/migrations ./migrations
COPY --from=web-build /src/apps/web/dist ./apps/web/dist
COPY dist/release ./agent-releases
COPY deploy/compose/entrypoint.sh /usr/local/bin/hope-entrypoint
RUN chmod 0755 /usr/local/bin/hope-entrypoint

ENV HOPE_WEB_DIST_DIR=/app/apps/web/dist
ENV HOPE_BIND_ADDR=0.0.0.0:8080
ENV HOPE_CA_CERT_PATH=/app/data/pki/ca-cert.pem
ENV HOPE_CA_KEY_PATH=/app/data/pki/ca-key.pem
ENV HOPE_SERVER_CERT_PATH=/app/data/pki/server-cert.pem
ENV HOPE_SERVER_KEY_PATH=/app/data/pki/server-key.pem
ENV HOPE_CREDENTIAL_MASTER_KEY_FILE=/app/data/pki/credential-master-key
ENV HOPE_AGENT_RELEASE_DIR=/app/agent-releases
ENV HOPE_RELEASE_PUBLIC_KEY_FILE=/app/agent-releases/public-key.hex
ENV HOPE_REQUIRE_BUNDLED_AGENTS=true
EXPOSE 8080 8443

ENTRYPOINT ["/usr/local/bin/hope-entrypoint"]
CMD ["serve"]
