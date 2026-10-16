.PHONY: check build run types test installer-tests release-tests docs-tests bench-agent shell-checks
types:
	./scripts/generate-types.sh
check:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets --all-features -- -D warnings
	cargo test --workspace
	cd web && npm run typecheck && npm run lint
shell-checks:
	for f in scripts/*.sh; do sh -n $$f || exit 1; done
	shellcheck -s sh scripts/*.sh
	for f in proxmox/ct/*.sh proxmox/install/*.sh; do bash -n $$f || exit 1; done
	shellcheck -s bash -e SC1090,SC1091,SC2154 proxmox/ct/*.sh proxmox/install/*.sh
installer-tests:
	for f in scripts/*.sh; do sh -n $$f || exit 1; done
	python3 -I -m unittest discover -s tests/installer -v
docs-tests:
	python3 -I -m unittest discover -s tests/docs -v
bench-agent:
	./scripts/bench-agent.sh
release-tests:
	python3 -I -m unittest discover -s tests/release -v
build: types
	cd web && npm ci && npm run build
	cargo build --release --locked --workspace
run:
	cargo run -p hope-server
test: check
	cd web && npm run build
	cargo build -p hope-server
	cd web && npm run test:e2e
