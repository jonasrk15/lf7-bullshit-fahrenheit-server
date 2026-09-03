FROM docker.io/library/rust:1-bookworm AS builder

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY static ./static
COPY migrations ./migrations

RUN cargo build --locked --release \
    && strip target/release/temperatur-server

FROM docker.io/library/debian:bookworm-slim AS runtime

LABEL org.opencontainers.image.source="https://github.com/jonasrk15/lf7-bullshit-fahrenheit-server" \
      org.opencontainers.image.description="Rust/Axum temperature server"

RUN mkdir -p /var/lib/temperatur-server \
    && chown 10001:10001 /var/lib/temperatur-server

COPY --from=builder /build/target/release/temperatur-server /usr/local/bin/temperatur-server
COPY --chmod=755 deploy/container-healthcheck.sh /usr/local/bin/container-healthcheck

ENV BIND_ADDR="0.0.0.0:3000" \
    DATABASE_URL="sqlite:///var/lib/temperatur-server/temperatures.db" \
    LEGACY_DATA_FILE="/var/lib/temperatur-server/data.json" \
    RUST_LOG="temperatur_server=info,tower_http=info"

USER 10001:10001
EXPOSE 3000

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD ["/usr/local/bin/container-healthcheck"]

ENTRYPOINT ["/usr/local/bin/temperatur-server"]
