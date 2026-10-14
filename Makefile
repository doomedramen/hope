.PHONY: check build run types test installer-tests
types:
	./scripts/generate-types.sh
check:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets --all-features -- -D warnings
	cargo test --workspace
	cd web && npm run typecheck && npm run lint
installer-tests:
	sh -n scripts/install-agent.sh
	python3 -I -m unittest discover -s tests/installer -v
build: types
	cd web && npm ci && npm run build
	cargo build --release --locked --workspace
run:
	cargo run -p hope-server
test: check
	cd web && npm run build
	cargo build -p hope-server
	cd web && npm run test:e2e
