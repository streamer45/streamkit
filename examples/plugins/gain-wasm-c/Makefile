# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

.PHONY: all compile clean

# Default WASI SDK path - override with `make WASI_SDK=/custom/path`
WASI_SDK ?= /opt/wasi-sdk

# Paths
SDK_DIR = ../../../plugin-sdk/c
BUILD_DIR = build
OUTPUT = $(BUILD_DIR)/gain_plugin_c.wasm

# SDK files (pre-generated)
SDK_H = $(SDK_DIR)/include/plugin.h
SDK_C = $(SDK_DIR)/src/plugin.c
SDK_O = $(SDK_DIR)/src/plugin_component_type.o

# Our implementation
IMPL_C = gain_plugin.c

all: $(OUTPUT)

# Compile the component
$(OUTPUT): $(IMPL_C) $(SDK_H) $(SDK_C) $(SDK_O)
	@echo "Compiling C plugin..."
	@mkdir -p $(BUILD_DIR)
	$(WASI_SDK)/bin/clang \
		--target=wasm32-wasip2 \
		-mexec-model=reactor \
		-Wl,--gc-sections \
		-Werror \
		-ffunction-sections \
		-fdata-sections \
		-I$(SDK_DIR)/include \
		-o $(OUTPUT) \
		$(IMPL_C) \
		$(SDK_C) \
		$(SDK_O)
	@echo "✓ Plugin built: $(OUTPUT)"

clean:
	@echo "Cleaning build artifacts..."
	@rm -rf $(BUILD_DIR)
