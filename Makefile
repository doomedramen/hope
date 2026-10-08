.PHONY: check build run types test
types:
	cargo run --quiet -p hope-server -- export-types > web/lib/protocol.ts
check:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets --all-features -- -D warnings
	cargo test --workspace
	cd web && npm run typecheck && npm run lint
build: types
	cd web && npm ci && npm run build
	cargo build --release --locked --workspace
run:
	cargo run -p hope-server
test: check
	cd web && npm run build
	cargo build -p hope-server
	cd web && npm run test:e2e
