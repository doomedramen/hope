FROM node:26-bookworm-slim AS web
WORKDIR /build/web
COPY web/package*.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:1.96-bookworm AS rust
RUN apt-get update && apt-get install -y --no-install-recommends cmake && rm -rf /var/lib/apt/lists/*
RUN rustup component add clippy rustfmt
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY scripts ./scripts
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked --workspace \
    && mkdir -p /build/dist \
    && cp target/release/hope-server target/release/hope-agent /build/dist/

FROM rust AS checks
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo fmt --all -- --check \
    && cargo test --workspace --locked \
    && cargo clippy --workspace --all-targets --all-features -- -D warnings

FROM checks AS load-checks
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo test --release --locked -p hope-server --test retention_load -- --ignored --nocapture \
    && cp /build/target/retention-benchmark.json /build/dist/retention-benchmark.json

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl libcap2-bin libc-bin && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 hope && useradd --uid 10001 --gid hope --no-create-home hope \
    && mkdir -p /var/lib/hope /opt/hope/releases && chown hope:hope /var/lib/hope
COPY --from=rust /build/dist/hope-server /usr/local/bin/hope-server
COPY --from=rust /build/dist/hope-agent /opt/hope/hope-agent-unsigned
RUN setcap cap_net_raw=ep /usr/local/bin/hope-server
COPY --from=web /build/web/out /opt/hope/web
ENV HOPE_DATA_DIR=/var/lib/hope HOPE_WEB_DIR=/opt/hope/web HOPE_RELEASE_DIR=/opt/hope/releases HOPE_BIND=0.0.0.0:8080
USER hope
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1
ENTRYPOINT ["hope-server"]
