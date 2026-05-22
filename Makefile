# lez-registry — Make Targets
#
# Built with nssa-framework (https://github.com/jimmy-claw/nssa-framework)
#
# Prerequisites:
#   - Rust stable toolchain
#   - risc0 toolchain (for `make build`)
#   - wallet CLI from lssa (for `make deploy`)
#   - Running sequencer (for `make deploy`)
#
# Quick start:
#   make build idl deploy
#   make cli ARGS="--help"

SHELL := /bin/bash
STATE_FILE := .registry-state
IDL_FILE := registry-idl.json
PROGRAMS_DIR := methods/guest/target/riscv32im-risc0-zkvm-elf/docker
REGISTRY_BIN := $(PROGRAMS_DIR)/registry.bin

-include $(STATE_FILE)

# ── Targets ──────────────────────────────────────────────────────────────────

.PHONY: help build build-ffi build-module check idl cli test deploy inspect status clean

help: ## Show this help
	@echo "lez-registry — Make Targets"
	@echo ""
	@echo "  make build              Build the zkVM guest binary (needs risc0 toolchain)"
	@echo "  make build-ffi          Build the lez-registry-ffi shared library (release)"
	@echo "  make build-module       Build the Qt spelbook-module (requires Qt6 + cmake)"
	@echo "  make check              cargo check host crates (no risc0 needed)"
	@echo "  make idl                Generate IDL JSON from #[nssa_program] annotations"
	@echo "  make cli ARGS=\"...\"     Run the IDL-driven CLI (pass args via ARGS=)"
	@echo "  make test               Run unit tests"
	@echo "  make deploy             Deploy to sequencer"
	@echo "  make inspect            Show ProgramId for built binary"
	@echo "  make status             Show saved state and binary info"
	@echo "  make clean              Remove saved state"
	@echo ""
	@echo "Example workflow:"
	@echo "  make build idl deploy"
	@echo "  make cli ARGS=\"--help\""
	@echo "  make cli ARGS=\"-p $(REGISTRY_BIN) register --name lez-multisig ...\""

build: ## Build the registry zkVM guest binary
	cargo risczero build --manifest-path methods/guest/Cargo.toml
	@echo ""
	@echo "✅ Guest binary built: $(REGISTRY_BIN)"
	@ls -la $(REGISTRY_BIN) 2>/dev/null || true

build-ffi: ## Build the lez-registry-ffi shared library (release, no risc0 needed)
	cargo build --release -p lez-registry-ffi
	@echo ""
	@echo "✅ FFI library built: target/release/liblez_registry_ffi.so"

build-module: build-ffi ## Build the Qt spelbook-module (requires Qt6 + cmake)
	cmake -S spelbook-module -B spelbook-module/build -DCMAKE_BUILD_TYPE=Release
	cmake --build spelbook-module/build
	@echo ""
	@echo "✅ Qt spelbook-module built: spelbook-module/build/"

check: ## Verify host crates compile (no risc0 toolchain needed)
	cargo check -p registry_core -p registry_program -p registry-cli
	cargo check -p lez-registry-ffi
	@echo ""
	@echo "✅ cargo check passed"

idl: ## Generate IDL JSON from #[nssa_program] annotations
	cargo run --bin generate_idl > $(IDL_FILE)
	@echo "✅ IDL written to $(IDL_FILE)"

cli: ## Run the IDL-driven CLI (ARGS="...")
	cargo run --bin registry -- -i $(IDL_FILE) $(ARGS)

test: ## Run unit tests
	cargo test -p registry_core -p registry_program
	@echo ""
	@echo "✅ Unit tests passed"

deploy: ## Deploy registry program to sequencer
	@test -f "$(REGISTRY_BIN)" || (echo "ERROR: Binary not found. Run 'make build' first."; exit 1)
	wallet deploy-program $(REGISTRY_BIN)
	@echo ""
	@echo "✅ Registry program deployed"

inspect: ## Show ProgramId for built binary
	cargo run --bin registry -- inspect $(REGISTRY_BIN)

status: ## Show saved state and binary info
	@echo "lez-registry Status"
	@echo "──────────────────────────────────────"
	@if [ -f "$(STATE_FILE)" ]; then cat $(STATE_FILE); else echo "(no state)"; fi
	@echo ""
	@echo "Binaries:"
	@ls -la $(REGISTRY_BIN) 2>/dev/null || echo "  registry.bin: NOT BUILT (run 'make build')"
	@echo ""
	@echo "IDL:"
	@ls -la $(IDL_FILE) 2>/dev/null || echo "  $(IDL_FILE): NOT GENERATED (run 'make idl')"

clean: ## Remove saved state
	rm -f $(STATE_FILE) $(STATE_FILE).tmp
	@echo "✅ State cleaned"
