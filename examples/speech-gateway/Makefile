.PHONY: build lint run clean

build:
	go build -o gateway ./cmd/gateway

lint:
	golangci-lint run

run:
	go run ./cmd/gateway

clean:
	rm -f gateway
