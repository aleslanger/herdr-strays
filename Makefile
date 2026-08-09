# strays — common tasks.
#
# `make` on its own lists what is available.

PLUGIN_ID := aleslanger.strays
BIN       := target/release/herdr-strays
HERDR     ?= herdr

.DEFAULT_GOAL := help
.PHONY: help build release test lint fmt fmt-check check audit link unlink install reinstall run clean

help: ## Show this help
	@grep -hE '^[a-z-]+:.*?## ' $(MAKEFILE_LIST) \
	  | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

build: ## Debug build
	cargo build

release: ## Optimised build — what the plugin runs
	cargo build --release

test: ## Unit and integration tests (needs git on PATH)
	cargo test

lint: ## Clippy, warnings treated as errors
	cargo clippy --all-targets -- -D warnings

fmt: ## Format
	cargo fmt

fmt-check: ## Fail if anything is unformatted
	cargo fmt --check

check: fmt-check lint test ## Everything CI would run

audit: ## Check dependencies against the RustSec advisory database
	@command -v cargo-audit >/dev/null 2>&1 || { \
	  echo "cargo-audit not installed: cargo install cargo-audit"; exit 1; }
	cargo audit

link: release ## Build, then register this checkout with herdr
	@# `plugin link` skips the [[build]] step, so the binary the manifest
	@# launches has to be put in place here.
	mkdir -p bin
	cp $(BIN) bin/herdr-strays
	$(HERDR) plugin link "$(CURDIR)"

unlink: ## Deregister the checkout, leaving files alone
	$(HERDR) plugin unlink $(PLUGIN_ID)

reinstall: unlink link ## Re-register after changing the manifest

run: release ## Run the viewer directly, outside herdr
	./$(BIN)

clean: ## Remove build artefacts
	cargo clean
