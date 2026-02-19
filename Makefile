.PHONY: build install test lint fmt-check ci clean

BINARY     := target/release/json-ls
INSTALL_PATH := $(HOME)/.local/bin/json-ls

build:
	cargo build --release

build-debug:
	cargo build

install: build
	install -Dm755 $(BINARY) $(INSTALL_PATH)
	@echo "Installed json-ls → $(INSTALL_PATH)"

test:
	cargo test

lint:
	cargo clippy -- -D warnings

fmt-check:
	cargo fmt --check

ci: fmt-check lint test build

clean:
	cargo clean
