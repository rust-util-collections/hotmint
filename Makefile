CARGO := cargo

.PHONY: all fmt lint build test run clean check doc

all: fmt lint build test

fmt:
	$(CARGO) fmt --all

lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

build:
	$(CARGO) build --workspace

test:
	$(CARGO) test --workspace

run:
	$(CARGO) run

check:
	$(CARGO) check --workspace --all-targets

doc:
	$(CARGO) doc --workspace --no-deps --open

clean:
	$(CARGO) clean
