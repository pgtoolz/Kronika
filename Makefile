RUST_TOOLCHAIN ?= 1.96.0
DYLINT_TOOLCHAIN ?= nightly-2026-05-28
TARGET ?= $(shell rustc +$(RUST_TOOLCHAIN) -vV | sed -n 's/^host: //p')
CARGO_BUILD = cargo +$(RUST_TOOLCHAIN) build --locked --target $(TARGET)
UI_DIR = bins/kronika-web/ui
REPORT_ASSET_FLAGS ?=

.PHONY: build collector demo report web ui-install ui-build ui-check report-assets report-assets-check fmt fmt-check query-boundary dylint lint test bdd-check check test-bdd demo-run demo-image demo-image-run demo-up demo-stop demo-clean demo-status demo-logs diagrams

build: ## Build every binary for the selected target.
	@$(CARGO_BUILD) -p kronika-collector -p kronika-dump -p kronika-demo -p kronika-report -p kronika-web

collector: ## Build kronika-collector.
	@$(CARGO_BUILD) -p kronika-collector

demo: ## Build kronika-demo.
	@$(CARGO_BUILD) -p kronika-demo

report: ## Build kronika-report from the committed browser assets.
	@$(CARGO_BUILD) -p kronika-report

web: ## Build kronika-web from the committed interface artifact.
	@$(CARGO_BUILD) -p kronika-web

ui-install: ## Install the locked interface build dependencies.
	@npm --prefix $(UI_DIR) ci

ui-build: ## Rebuild the committed self-contained interface artifact.
	@RUST_TOOLCHAIN=$(RUST_TOOLCHAIN) npm --prefix $(UI_DIR) run build

ui-check: ## Type-check and reproduce the committed interface artifact.
	@npm --prefix $(UI_DIR) run fixture:check
	@npm --prefix $(UI_DIR) test
	@npm --prefix $(UI_DIR) run typecheck
	@RUST_TOOLCHAIN=$(RUST_TOOLCHAIN) npm --prefix $(UI_DIR) run check

report-assets: ui-build ## Rebuild the committed report shell and WebAssembly bindings.
	@RUST_TOOLCHAIN=$(RUST_TOOLCHAIN) scripts/report-assets.sh build $(REPORT_ASSET_FLAGS)

report-assets-check: ui-check ## Reproduce the committed report shell and WebAssembly bindings.
	@RUST_TOOLCHAIN=$(RUST_TOOLCHAIN) scripts/report-assets.sh check $(REPORT_ASSET_FLAGS)

fmt: ## Format the product and nested lint workspaces.
	@cargo +$(RUST_TOOLCHAIN) fmt --all
	@cargo +$(RUST_TOOLCHAIN) fmt --manifest-path lints/kronika_lints/Cargo.toml --all
	@cargo +$(RUST_TOOLCHAIN) fmt --manifest-path lints/kronika_lints/fixtures/Cargo.toml --all

fmt-check: ## Verify product and nested lint workspace formatting.
	@cargo +$(RUST_TOOLCHAIN) fmt --all --check
	@cargo +$(RUST_TOOLCHAIN) fmt --manifest-path lints/kronika_lints/Cargo.toml --all --check
	@cargo +$(RUST_TOOLCHAIN) fmt --manifest-path lints/kronika_lints/fixtures/Cargo.toml --all --check

query-boundary: ## Verify the shared query layer remains storage and transport neutral.
	@scripts/check-query-boundary.sh

dylint: ## Run the pinned repository and Mordant lint set.
	@DYLINT_TOOLCHAIN=$(DYLINT_TOOLCHAIN) scripts/check-dylints.sh

lint: query-boundary dylint ## Run repository-specific lints and clippy with warnings denied.
	@cargo +$(RUST_TOOLCHAIN) clippy --locked --workspace --all-targets -- -D warnings

test: ## Run every non-BDD unit and integration test.
	@cargo +$(RUST_TOOLCHAIN) test --locked --workspace --exclude kronika-bdd --all-targets

bdd-check: ## Compile the BDD runner and the binaries it exercises, without running scenarios.
	@cargo +$(RUST_TOOLCHAIN) build --locked -p kronika-collector -p kronika-dump -p kronika-bdd

check: report-assets-check fmt-check lint test bdd-check ## The full local pre-commit gate.

test-bdd: ## Run BDD inside the cached Docker image.
	@scripts/bdd-image.sh run

demo-run: ## Run the collector for a bounded window and report its cost.
	@$(CARGO_BUILD) -p kronika-collector -p kronika-demo
	@KRONIKA_COLLECTOR_BIN=target/$(TARGET)/debug/kronika-collector \
		target/$(TARGET)/debug/kronika-demo

demo-image: ## Build the demo Docker image (PostgreSQL, PgBouncer, collector, web).
	@scripts/demo-image.sh build

demo-image-run: ## Build and run the demo image, publishing web on :8080.
	@scripts/demo-image.sh up

demo-up: ## Build and start the interactive demo, then wait until it is usable.
	@scripts/demo-image.sh up

demo-stop: ## Stop the demo while preserving collected demo data.
	@scripts/demo-image.sh stop

demo-clean: ## Stop the demo and remove its containers, network, and data volume.
	@scripts/demo-image.sh clean

demo-status: ## Show the demo container and health state.
	@scripts/demo-image.sh status

demo-logs: ## Follow all demo service logs.
	@scripts/demo-image.sh logs

DRAWIO ?= drawio

diagrams: ## Regenerate the committed documentation SVGs from docs/diagrams (requires the draw.io CLI).
	@for f in docs/diagrams/*.drawio; do \
		"$(DRAWIO)" --export --format svg --theme light --border 16 \
			--output docs/images/$$(basename $$f .drawio).svg $$f; \
	done
