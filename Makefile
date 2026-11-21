.PHONY: build binary appimage all install test check clean

build:
	cargo build --release --locked

binary: build

appimage:
	./scripts/build-appimage.sh

all: build
	./scripts/build-appimage.sh --skip-build

install:
	cargo install --path . --locked

test:
	cargo test --workspace

check:
	cargo fmt --all -- --check
	cargo check --workspace --all-targets
	cargo test --workspace
	cargo clippy --workspace --all-targets --all-features -- -D warnings

clean:
	rm -rf dist
	cargo clean
