CARGO := cargo

# Publish order (topological by internal deps)
CRATES := \
	hotmint-types \
	hotmint-mempool \
	hotmint-crypto \
	hotmint-consensus \
	hotmint-abci \
	hotmint-network \
	hotmint-storage \
	hotmint-api \
	hotmint

.PHONY: all fmt lint build test bench bench-e2e bench-consensus bench-evm bench-all run clean check doc update publish

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

bench-consensus:
	$(CARGO) run --release --bin bench-consensus

bench-evm:
	$(CARGO) run --release -p evm-chain-example --bin bench-evm

bench-all: bench-consensus bench-evm

run:
	$(CARGO) run --bin hotmint-node -- node

demo:
	$(CARGO) run --bin hotmint-demo

init:
	$(CARGO) run --bin hotmint-node -- init

check:
	$(CARGO) check --workspace --all-targets

doc:
	$(CARGO) doc --workspace --no-deps --open

clean:
	$(CARGO) clean

update:
	$(CARGO) update

publish:
	@for crate in $(CRATES); do \
		printf "Publishing $$crate... "; \
		output=$$($(CARGO) publish -p $$crate 2>&1); \
		status=$$?; \
		if [ $$status -eq 0 ]; then \
			echo "ok"; \
			sleep 2; \
		elif echo "$$output" | grep -qE "already uploaded|already exists"; then \
			echo "skipped (already published)"; \
		else \
			echo "FAILED"; \
			echo "$$output"; \
			exit 1; \
		fi; \
	done
