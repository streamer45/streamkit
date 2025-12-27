# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

# StreamKit Gain Plugin - C Implementation
# Makefile for building the shared library

# Compiler settings
CC ?= gcc
CFLAGS = -Wall -Wextra -Werror -pedantic -std=c11 -fPIC -O2
LDFLAGS = -shared -lm

# Platform detection
UNAME := $(shell uname)
ifeq ($(UNAME), Darwin)
    # macOS
    LIB_EXT = dylib
    LIB_PREFIX = lib
    LDFLAGS += -dynamiclib
else ifeq ($(OS), Windows_NT)
    # Windows
    LIB_EXT = dll
    LIB_PREFIX =
else
    # Linux and others
    LIB_EXT = so
    LIB_PREFIX = lib
endif

# Output
BUILD_DIR = build
TARGET = $(BUILD_DIR)/$(LIB_PREFIX)gain_plugin_c.$(LIB_EXT)

# Sources
SOURCES = gain_plugin.c
OBJECTS = $(SOURCES:%.c=$(BUILD_DIR)/%.o)

# Default target
.PHONY: all
all: $(TARGET)

# Create build directory
$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

# Compile object files
$(BUILD_DIR)/%.o: %.c | $(BUILD_DIR)
	$(CC) $(CFLAGS) -c $< -o $@

# Link shared library
$(TARGET): $(OBJECTS)
	$(CC) $(OBJECTS) -o $@ $(LDFLAGS)
	@echo "Built: $(TARGET)"

# Clean build artifacts
.PHONY: clean
clean:
	rm -rf $(BUILD_DIR)

# Install to StreamKit plugins directory
.PHONY: install
install: $(TARGET)
	mkdir -p ../../../.plugins/native
	cp $(TARGET) ../../../.plugins/native/
	@echo "Installed to .plugins/native/"

# Debug build with symbols
.PHONY: debug
debug: CFLAGS += -g -O0 -DDEBUG
debug: clean $(TARGET)

# Show help
.PHONY: help
help:
	@echo "StreamKit Gain Plugin (C) - Build targets:"
	@echo ""
	@echo "  make          - Build the plugin (release)"
	@echo "  make debug    - Build with debug symbols"
	@echo "  make clean    - Remove build artifacts"
	@echo "  make install  - Copy plugin to .plugins/native/"
	@echo "  make help     - Show this help"
	@echo ""
	@echo "Output: $(TARGET)"
