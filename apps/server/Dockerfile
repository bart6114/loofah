FROM rust:1.94-bookworm AS builder
WORKDIR /src
COPY . .
RUN --mount=type=cache,id=loofah-server-registry,target=/usr/local/cargo/registry --mount=type=cache,id=loofah-server-git,target=/usr/local/cargo/git --mount=type=cache,id=loofah-server-target,target=/src/target cargo build --locked --release -p loofah-server && cp target/release/loofah-server /tmp/loofah-server

FROM builder AS verification
RUN --mount=type=cache,id=loofah-server-registry,target=/usr/local/cargo/registry --mount=type=cache,id=loofah-server-git,target=/usr/local/cargo/git --mount=type=cache,id=loofah-server-target,target=/src/target cargo test --locked -p loofah-server -p vault-access && cargo clippy --locked -p loofah-server -p vault-access --all-targets --no-deps -- -D warnings

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* && useradd --system --uid 10001 --create-home loofah && mkdir /data /config && chown loofah:loofah /data
COPY --from=builder /tmp/loofah-server /usr/local/bin/loofah-server
USER loofah
VOLUME ["/data"]
EXPOSE 8947
ENTRYPOINT ["loofah-server", "--config", "/config/server.json"]
CMD ["serve", "--listen", "0.0.0.0:8947"]
