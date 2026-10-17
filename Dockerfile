ARG RUST_VERSION=1.96.0

FROM node:26-bookworm-slim AS web
WORKDIR /build/web
COPY web/package*.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:${RUST_VERSION}-bookworm AS rust-source
RUN apt-get update && apt-get install -y --no-install-recommends cmake && rm -rf /var/lib/apt/lists/*
RUN rustup component add clippy rustfmt
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY scripts ./scripts

FROM rust-source AS rust
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked --workspace \
    && mkdir -p /build/dist \
    && cp target/release/hope-server target/release/hope-agent /build/dist/

FROM rust-source AS checks
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo fmt --all -- --check \
    && cargo test --workspace --locked \
    && cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

FROM checks AS load-checks
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo test --release --locked -p hope-server --test retention_load -- --ignored --nocapture \
    && mkdir -p /build/dist \
    && cp /build/target/retention-benchmark.json /build/dist/retention-benchmark.json

# The installer harness relies on Linux tools and uname, even when hooks run on macOS.
FROM debian:bookworm-slim AS script-checks
RUN apt-get update && apt-get install -y --no-install-recommends python3 openssl make ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Makefile Cargo.toml ./
COPY scripts ./scripts
COPY install ./install
COPY ct ./ct
COPY json ./json
COPY .github ./.github
COPY crates ./crates
COPY tests/installer ./tests/installer
COPY tests/release ./tests/release
RUN make installer-tests release-tests

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl libcap2-bin libc-bin && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 hope && useradd --uid 10001 --gid hope --no-create-home hope \
    && mkdir -p /var/lib/hope /opt/hope/releases && chown hope:hope /var/lib/hope
COPY --from=rust /build/dist/hope-server /usr/local/bin/hope-server
COPY --from=rust /build/dist/hope-agent /opt/hope/hope-agent-unsigned
# The binary carries a file capability, so the container's bounding set MUST include NET_RAW
# (compose `cap_add: [NET_RAW]`); without it exec fails with "operation not permitted". Do not set
# no-new-privileges. The process still runs as the unprivileged user hope.
RUN setcap cap_net_raw=ep /usr/local/bin/hope-server
COPY --from=web /build/web/out /opt/hope/web
ENV HOPE_DATA_DIR=/var/lib/hope HOPE_WEB_DIR=/opt/hope/web HOPE_RELEASE_DIR=/opt/hope/releases HOPE_BIND=0.0.0.0:8080
USER hope
EXPOSE 8080
# Probe the configured listener (a wildcard bind is reached through loopback).
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD ["sh", "-c", "b=${HOPE_BIND:-127.0.0.1:8080}; h=${b%:*}; case \"$h\" in 0.0.0.0|\"[::]\"|\"\") h=127.0.0.1;; esac; curl -fsS --max-time 2 \"http://$h:${b##*:}/healthz\" >/dev/null"]
ENTRYPOINT ["hope-server"]
