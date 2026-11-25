# AI-Native OS Orchestrator - Justfile
# Run `just --list` to see all available commands

# Default shell
set shell := ["bash", "-cu"]

# Variables
CARGO := cargo
NIX := nix
HOSTNAME := `hostname`

# --- Build ---
build:
	@echo "Building workspace..."
	$(CARGO) build --workspace --locked

build-release:
	@echo "Building release..."
	$(CARGO) build --release --workspace --locked

# --- Test ---
test:
	@echo "Running tests..."
	$(CARGO) test --workspace --locked

test-nextest:
	@echo "Running tests with nextest..."
	$(CARGO) nextest run --workspace --locked

test-integration:
	@echo "Running integration tests..."
	./scripts/test-integration.sh

# --- Lint/Format ---
fmt:
	@echo "Formatting..."
	$(CARGO) fmt --all

clippy:
	@echo "Running clippy..."
	$(CARGO) clippy --workspace --locked -- -D warnings

audit:
	@echo "Running cargo audit..."
	$(CARGO) audit --workspace || true

check: fmt clippy test
	@echo "All checks passed!"

# --- Nix ---
nix-build:
	@echo "Building with Nix..."
	$(NIX) build .#orchestrator --no-link

nix-check:
	@echo "Checking flake..."
	$(NIX) flake check --no-build

nix-rebuild:
	@echo "Rebuilding NixOS configuration for $(HOSTNAME)..."
	sudo nixos-rebuild switch --flake .#$(HOSTNAME)

nix-rebuild-dry:
	@echo "Dry-run NixOS rebuild..."
	sudo nixos-rebuild dry-build --flake .#$(HOSTNAME)

nix-gc:
	@echo "Garbage collecting..."
	sudo nix-collect-garbage -d

# --- Run ---
run:
	@echo "Running orchestrator..."
	./target/release/orchestrator --config /etc/orchestrator/config.toml

run-dev:
	@echo "Running orchestrator (dev)..."
	RUST_LOG=debug ./target/debug/orchestrator --config config.example.toml

tui:
	@echo "Starting TUI..."
	./target/release/orchestrator-tui

# --- Systemd ---
service-start:
	sudo systemctl start orchestrator

service-stop:
	sudo systemctl stop orchestrator

service-restart:
	sudo systemctl restart orchestrator

service-status:
	systemctl status orchestrator

service-logs:
	journalctl -u orchestrator -f

service-enable:
	sudo systemctl enable orchestrator

service-disable:
	sudo systemctl disable orchestrator

# --- Bootstrap ---
bootstrap:
	@echo "Bootstrapping AI-Native OS..."
	sudo ./scripts/bootstrap.sh

bootstrap-phone:
	@echo "Bootstrapping for phone..."
	sudo HOSTNAME=phone ./scripts/bootstrap.sh

# --- Dev Shell ---
dev-shell:
	@echo "Entering dev shell..."
	nix develop

direnv-allow:
	direnv allow

# --- Clean ---
clean:
	@echo "Cleaning build artifacts..."
	$(CARGO) clean
	rm -rf target/

clean-all: clean
	@echo "Cleaning Nix store..."
	sudo nix-collect-garbage -d

# --- Git ---
git-status:
	git status

git-diff:
	git diff

git-commit:
	@read -p "Commit message: " msg; \
	git add -A && git commit -m "$$msg"

git-push:
	git push

# --- Model ---
download-model:
	@echo "Downloading Qwen2.5-Coder-32B-4bit..."
	mkdir -p /var/lib/orchestrator/models
	curl -L -o /var/lib/orchestrator/models/qwen2.5-coder-32b-4bit.gguf \
		https://huggingface.co/Qwen/Qwen2.5-Coder-32B-Instruct-GGUF/resolve/main/qwen2.5-coder-32b-instruct-q4_k_m.gguf

download-model-phone:
	@echo "Downloading Qwen2.5-Coder-7B-4bit..."
	mkdir -p /var/lib/orchestrator/models
	curl -L -o /var/lib/orchestrator/models/qwen2.5-coder-7b-4bit.gguf \
		https://huggingface.co/Qwen/Qwen2.5-Coder-7B-Instruct-GGUF/resolve/main/qwen2.5-coder-7b-instruct-q4_k_m.gguf

# --- Federation ---
federation-sync:
	@echo "Syncing federation..."
	sudo -u orchestrator /run/current-system/sw/bin/orchestrator-federate sync

federation-pull:
	@echo "Pulling federation updates..."
	sudo -u orchestrator /run/current-system/sw/bin/orchestrator-federate pull

# --- Policy ---
policy-test:
	@echo "Testing policy engine..."
	./target/release/orchestrator-policy-test

# --- Help ---
help:
	@just --list

.PHONY: build build-release test test-nextest test-integration fmt clippy audit check \
        nix-build nix-check nix-rebuild nix-rebuild-dry nix-gc \
        run run-dev tui \
        service-start service-stop service-restart service-status service-logs service-enable service-disable \
        bootstrap bootstrap-phone dev-shell direnv-allow \
        clean clean-all \
        git-status git-diff git-commit git-push \
        download-model download-model-phone \
        federation-sync federation-pull policy-test help