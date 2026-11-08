FROM rust:1.94-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends cmake clang autoconf automake libtool pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY . .
RUN cargo build --locked --release -p loofah-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* && mkdir -p /var/lib/loofah /var/cache/loofah && chown 10001:10001 /var/lib/loofah /var/cache/loofah
COPY --from=builder /build/target/release/loofah-server /usr/local/bin/loofah-server
USER 10001:10001
EXPOSE 8027
HEALTHCHECK --interval=30s --timeout=5s CMD curl --fail --silent http://127.0.0.1:8027/ready || exit 1
ENTRYPOINT ["loofah-server"]
CMD ["--config", "/etc/loofah/server.json"]
