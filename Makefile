# Makefile for common tasks in a Rust project
# Detect current branch
CURRENT_BRANCH := $(shell git rev-parse --abbrev-ref HEAD)
ZIP_NAME = PriceLevel.zip


# Default target
.PHONY: all
all: test fmt lint build

# Build the project
.PHONY: build
build:
	cargo build

.PHONY: release
release:
	cargo build --release

# Run tests
.PHONY: test
test:
	LOGLEVEL=WARN cargo test

# Format the code
.PHONY: fmt
fmt:
	cargo +stable fmt --all

# Check formatting
.PHONY: fmt-check
fmt-check:
	cargo +stable fmt --check

# Run Clippy for linting, plus the Production Panic Policy syntax gate
# (issue #173): clippy's `[lints.clippy]` restriction lints (Cargo.toml) and
# this crate's `clippy.toml` cover unwrap/expect/panic/unreachable/todo/
# unimplemented/indexing/string-slicing/narrowing-casts/raw-arithmetic in
# production. `lint-panic` below covers what clippy has NO lint for at all
# (the `assert!`/`debug_assert!` family) and what clippy's own
# `#[cfg(test)]` heuristic can wrongly exempt (a standalone `#[cfg(test)]`
# production helper that is not a `mod tests { ... }` block), plus
# `saturating_*`/`wrapping_*` on production state.
.PHONY: lint
lint: lint-panic
	cargo clippy --all-targets --all-features -- -D warnings

# Production Panic Policy syntax gate (issue #173): scripts/check_panic_policy.py.
# Runs the scanner's own fixture self-test first — a broken scanner must
# never silently report a clean src/ — then scans src/ for real.
.PHONY: lint-panic
lint-panic:
	python3 scripts/check_panic_policy.py --self-test
	python3 scripts/check_panic_policy.py

.PHONY: lint-fix
lint-fix:
	# `-A clippy::manual_saturating_arithmetic`: `cargo clippy --fix` has
	# rewritten checked arithmetic into `saturating_*` before (issue #173,
	# reported against #178) — this crate never wants that rewrite auto-
	# applied. `lint-panic` re-scans src/ for saturating_*/wrapping_*
	# afterward as an independent, non-autofix-dependent check.
	cargo clippy --fix --all-targets --all-features --allow-dirty --allow-staged \
		-- -D warnings -A clippy::manual_saturating_arithmetic
	$(MAKE) lint-panic

# Clean the project
.PHONY: clean
clean:
	cargo clean

# Pre-push checks
.PHONY: check
check: test fmt-check lint

# Run the project
.PHONY: run
run:
	cargo run

.PHONY: fix
fix:
	cargo fix --allow-staged --allow-dirty

.PHONY: pre-push
# Ordering (issue #173, review comment on #178): `lint-fix` runs BEFORE
# `fmt`, not after. `cargo clippy --fix` can rewrite code (including, before
# `-A clippy::manual_saturating_arithmetic` above, into `saturating_*`)
# without reformatting it, so running `fmt` first and `lint-fix` last used to
# leave the tree unformatted after a "clean" pre-push. `lint-panic` runs
# after `fmt` since it is a plain text scan unaffected by formatting, and
# before `test` so a Production Panic Policy regression fails fast.
pre-push: fix lint-fix fmt lint-panic test readme doc

.PHONY: doc
doc:
	cargo clippy -- -W missing-docs
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items

.PHONY: doc-open
doc-open:
	cargo doc --open

.PHONY: publish
publish: readme
	find . -name ".DS_Store" -type f -delete | true
	cargo login ${CARGO_REGISTRY_TOKEN}
	cargo package
	cargo publish

.PHONY: coverage
coverage:
	export LOGLEVEL=WARN
	cargo install cargo-tarpaulin
	mkdir -p coverage
	cargo tarpaulin --exclude-files 'benches/**' --all-features --workspace --timeout 120 --out Xml

.PHONY: coverage-html
coverage-html:
	export LOGLEVEL=WARN
	cargo install cargo-tarpaulin
	mkdir -p coverage
	cargo tarpaulin --exclude-files 'benches/**' --verbose --all-features --workspace --timeout 120 --out Html

.PHONY: open-coverage
open-coverage:
	open tarpaulin-report.html

# Rule to show git log
git-log:
	@if [ "$(CURRENT_BRANCH)" = "HEAD" ]; then \
		echo "You are in a detached HEAD state. Please check out a branch."; \
		exit 1; \
	fi; \
	echo "Showing git log for branch $(CURRENT_BRANCH) against main:"; \
	git log main..$(CURRENT_BRANCH) --pretty=full

.PHONY: create-doc
create-doc:
	cargo doc --no-deps --document-private-items

.PHONY: readme
readme: check-cargo-readme create-doc
	cargo readme > README.md

.PHONY: check-cargo-readme
check-cargo-readme:
	@command -v cargo-readme > /dev/null || (echo "Installing cargo-readme..."; cargo install cargo-readme)

.PHONY: check-spanish
check-spanish:
	cd scripts && python3 spanish.py ../src && cd ..

.PHONY: zip
zip:
	@echo "Creating $(ZIP_NAME) without any 'target' directories, 'Cargo.lock', and hidden files..."
	@find . -type f \
		! -path "*/target/*" \
		! -path "./.*" \
		! -name "Cargo.lock" \
		! -name ".*" \
		| zip -@ $(ZIP_NAME)
	@echo "$(ZIP_NAME) created successfully."


.PHONY: check-cargo-criterion
check-cargo-criterion:
	@command -v cargo-criterion > /dev/null || (echo "Installing cargo-criterion..."; cargo install cargo-criterion)

## NOTE: every `cargo criterion` target below pins `--bench benches` explicitly.
## Registering the separate `latency` bench target (issue #142, `[[bench]] name
## = "latency"` in Cargo.toml) means an unfiltered `cargo criterion` with no
## `--bench` argument would pick up BOTH bench targets and run the latency
## harness too — slowing down every Criterion invocation and mixing its
## `#[global_allocator]` process into the same run. `make bench-latency` below
## is the only entrypoint for the latency harness (issue #142 review finding 9).
.PHONY: bench
bench: check-cargo-criterion
	cargo criterion --bench benches --output-format=quiet

.PHONY: bench-show
bench-show:
	open target/criterion/report/index.html

.PHONY: bench-save
bench-save: check-cargo-criterion
	cargo criterion --bench benches --output-format quiet --history-id v0.3.2 --history-description "Version 0.3.2 baseline"

.PHONY: bench-compare
bench-compare: check-cargo-criterion
	cargo criterion --bench benches --output-format verbose

.PHONY: bench-json
bench-json: check-cargo-criterion
	cargo criterion --bench benches --message-format json

.PHONY: bench-clean
bench-clean:
	rm -rf target/criterion

# Isolated operation / tail-latency harness (issue #142) — a separate,
# harness=false bench target from `bench` above; see `benches/latency/main.rs`.
# Every knob is an env var (`benches/latency/config.rs`), e.g. a short
# validation run:
#   PL_LATENCY_SAMPLES=200 PL_LATENCY_WARMUP=50 PL_LATENCY_CONTENTION_OPS=200 \
#     PL_LATENCY_ALLOC_REPS=200 make bench-latency
.PHONY: bench-latency
bench-latency:
	cargo bench --bench latency


.PHONY: workflow-coverage
workflow-coverage:
	DOCKER_HOST="$${DOCKER_HOST}" act push --job code_coverage_report \
       -P ubuntu-latest=catthehacker/ubuntu:latest \
       --privileged

.PHONY: workflow-build
workflow-build:
	DOCKER_HOST="$${DOCKER_HOST}" act push --job build \
       -P ubuntu-latest=catthehacker/ubuntu:latest

.PHONY: workflow-lint
workflow-lint:
	DOCKER_HOST="$${DOCKER_HOST}" act push --job lint

.PHONY: workflow-test
workflow-test:
	DOCKER_HOST="$${DOCKER_HOST}" act push --job run_tests

.PHONY: workflow
workflow: workflow-build workflow-lint workflow-test workflow-coverage

.PHONY: integration-examples
integration-examples:
	cargo run --package examples --bin integration_basic_lifecycle
	cargo run --package examples --bin integration_trade_roundtrip
	cargo run --package examples --bin integration_newtypes_contract
	cargo run --package examples --bin integration_special_orders
	cargo run --package examples --bin integration_snapshot_recovery
	cargo run --package examples --bin integration_checked_arithmetic

# Run EVERY example binary (the assertion-bearing integration_* set plus the
# standalone demos) in a debug build so runtime asserts / debug_assert! fire.
# This is what catches example regressions that `cargo test` / `cargo build`
# alone miss (e.g. a stale snapshot-version assertion or a self-fill).
.PHONY: run-examples
run-examples: integration-examples
	cargo run --package examples --bin simple
	cargo run --package examples --bin hft_simulation
	cargo run --package examples --bin contention_test

.PHONY: tree
tree: 
	tree -I 'target|.idea|.run|.DS_Store|Cargo.lock|*.md|*.toml|*.zip|*.html|*.xml|*.json|*.txt|*.sh|*.yml|*.yaml|*.gitignore|*.gitattributes|*.gitmodules|*.git|*.gitkeep|*.gitlab-ci.yml' -a -L 3