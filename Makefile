CARGO := cargo

# System rocksdb (required by vsdb)
export ROCKSDB_INCLUDE_DIR ?= /opt/homebrew/include
export ROCKSDB_LIB_DIR ?= /opt/homebrew/lib

.PHONY: all fmt lint build test bench bench-e2e run clean check doc

all: fmt lint build test

fmt:
	$(CARGO) fmt --all

lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

build:
	$(CARGO) build --workspace

test:
	$(CARGO) test --workspace

bench:
	$(CARGO) bench --workspace

bench-e2e:
	$(CARGO) run --release --bin hotmint-bench-e2e

run:
	$(CARGO) run --bin hotmint-node

check:
	$(CARGO) check --workspace --all-targets

doc:
	$(CARGO) doc --workspace --no-deps --open

clean:
	$(CARGO) clean
