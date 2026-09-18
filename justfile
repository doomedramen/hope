set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

fmt:
    cargo fmt --all
    pnpm -C apps/web format

lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    pnpm -C apps/web lint

test:
    cargo test --workspace
    pnpm -C apps/web test -- --run

dev:
    cargo run -p server -- serve

web:
    pnpm -C apps/web dev

migrate:
    cargo run -p server -- migrate
