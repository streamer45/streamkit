# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

FROM golang:1.24-bookworm AS build
WORKDIR /src
COPY . .
RUN CGO_ENABLED=0 go build -o /gateway ./cmd/gateway

FROM gcr.io/distroless/static-debian12
COPY --from=build /gateway /gateway
EXPOSE 8080
ENTRYPOINT ["/gateway"]
