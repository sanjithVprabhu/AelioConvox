# Multi-stage build for ll-server: compile in a full Rust image, ship a slim runtime.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
# rustls-tls (no OpenSSL) keeps the build self-contained.
RUN cargo build --release -p ll-server

FROM debian:bookworm-slim AS runtime
# ca-certificates is needed for outbound TLS (the natural-language endpoint calls the
# Anthropic API over HTTPS); the engine itself has no other runtime dependencies.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/ll-server /usr/local/bin/ll-server

# Persisted data lives here — mount a volume to keep it across container restarts.
ENV LL_DATA_DIR=/data
ENV LL_BIND=0.0.0.0:8080
VOLUME ["/data"]
EXPOSE 8080

# Configure auth and (optionally) NL at run time:
#   docker run -p 8080:8080 -v ll-data:/data \
#     -e LL_API_KEYS=secret -e ANTHROPIC_API_KEY=sk-... ll-server
ENTRYPOINT ["ll-server"]
