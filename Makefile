.DEFAULT_GOAL := build

.PHONY: build debug menu test check fmt fmt-check help

build:
	cargo build --release

debug:
	cargo build

ifeq ($(shell uname -s),Darwin)
menu:
	sh src/client/menu/bundle-macos.sh
else
menu: build
endif

test:
	cargo test

check:
	cargo check --all-targets

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

help:
	@printf '%s\n' \
	  'make build      Build the release binary (default)' \
	  'make debug      Build the debug binary' \
	  'make menu       Package the macOS app or build the Linux release binary' \
	  'make test       Run tests' \
	  'make check      Check all Cargo targets' \
	  'make fmt        Format Rust code' \
	  'make fmt-check  Check Rust formatting'
