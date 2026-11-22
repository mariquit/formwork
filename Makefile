.DEFAULT_GOAL := build

.PHONY: clean test build install package

CARGO := cargo
PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin

clean:
	$(CARGO) clean

# Full gate. clippy runs before the build because it evicts
# target/release/formwork... and target/debug/formwork, which the suite
# shells out to — so the build must come after it, not before.
test:
	$(CARGO) fmt --check
	$(CARGO) clippy --locked --all-targets -- -D warnings
	$(CARGO) build --locked
	$(CARGO) test --locked
	node --test opencode-plugin/tools.test.mjs

build:
	$(CARGO) build --locked --release --target-dir target

install: build
	BINDIR="$(BINDIR)" sh scripts/install.sh target/release

package: build
	sh scripts/package.sh target/release target/dist
