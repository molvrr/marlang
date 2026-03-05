MARLANG_HOME ?= $(HOME)/.marlang
BIN_DIR      := $(MARLANG_HOME)/bin
STDLIB_DIR   := $(MARLANG_HOME)/stdlib
CARGO_TARGET := port/target/release/mar

.PHONY: build install uninstall clean

build:
	cargo build --release --manifest-path port/Cargo.toml -p marlang-cli

install: build
	@mkdir -p $(BIN_DIR) $(STDLIB_DIR)
	cp $(CARGO_TARGET) $(BIN_DIR)/mar
	@if command -v codesign >/dev/null 2>&1; then codesign --force --sign - $(BIN_DIR)/mar; fi
	cp stdlib/*.mar $(STDLIB_DIR)/
	@echo ""
	@echo "Installed to $(MARLANG_HOME)"
	@echo "  binary: $(BIN_DIR)/mar"
	@echo "  stdlib: $(STDLIB_DIR)/"
	@echo ""
	@echo "Add the following to your shell profile:"
	@echo "  export MARLANG_HOME=$(MARLANG_HOME)"
	@echo "  export PATH=\"$(BIN_DIR):\$$PATH\""

uninstall:
	rm -f $(BIN_DIR)/mar
	rm -rf $(STDLIB_DIR)
	@rmdir $(BIN_DIR) 2>/dev/null || true
	@rmdir $(MARLANG_HOME) 2>/dev/null || true
	@echo "Uninstalled from $(MARLANG_HOME)"

clean:
	cargo clean --manifest-path port/Cargo.toml
