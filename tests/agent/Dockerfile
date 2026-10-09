# Build the agent for the selected Linux platform without running a server build.
FROM rust:1.96-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
ARG TARGETARCH
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target,id=hope-agent-${TARGETARCH} \
    cargo build --release --locked -p hope-agent \
    && mkdir -p /dist && cp target/release/hope-agent /dist/hope-agent
FROM build AS checks
RUN rustup component add rustfmt clippy
ARG TARGETARCH
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target,id=hope-agent-${TARGETARCH} \
    cargo fmt --all -- --check \
    && cargo test --locked -p hope-agent \
    && cargo clippy --locked -p hope-agent --all-targets --all-features -- -D warnings
FROM scratch AS artifact
COPY --from=build /dist/hope-agent /hope-agent
