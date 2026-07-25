# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

.PHONY: build lint test run clean

build:
	go build -o gateway ./cmd/gateway

lint:
	golangci-lint run

test:
	go test ./...

run:
	go run ./cmd/gateway

clean:
	rm -f gateway
