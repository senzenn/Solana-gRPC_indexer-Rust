FROM rust:1-bookworm AS build
# build.rs compiles src/proto with prost-build, which needs protoc on PATH.
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
RUN cargo build --release --bin index

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --user-group --home-dir /app --shell /usr/sbin/nologin indexer \
    && mkdir -p /data && chown indexer:indexer /data
COPY --from=build /app/target/release/index /usr/local/bin/index
COPY --from=build /app/idls /app/idls
COPY --from=build /app/public /app/public
WORKDIR /app
ENV DATABASE_URL=sqlite:/data/index.db
USER indexer
EXPOSE 8080
ENTRYPOINT ["index"]
# 0.0.0.0 is needed inside the container; compose publishes the port on host loopback only.
CMD ["run", "--idl-dir", "/app/idls", "--bind", "0.0.0.0:8080", "--db", "sqlite:/data/index.db"]
