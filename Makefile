# strays — common tasks.
#
# `make` on its own lists what is available.

PLUGIN_ID := aleslanger.strays
BIN       := target/release/herdr-strays
HERDR     ?= herdr

.DEFAULT_GOAL := help
.PHONY: help build release test lint fmt fmt-check check link unlink install reinstall run clean

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

link: release ## Build, then register this checkout with herdr
	$(HERDR) plugin link "$(CURDIR)"

unlink: ## Deregister the checkout, leaving files alone
	$(HERDR) plugin unlink $(PLUGIN_ID)

reinstall: unlink link ## Re-register after changing the manifest

run: release ## Run the viewer directly, outside herdr
	./$(BIN)

clean: ## Remove build artefacts
	cargo clean
