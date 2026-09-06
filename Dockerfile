FROM rust:1-bookworm AS build
WORKDIR /app
COPY . .
RUN cargo build --release --bin index

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/index /usr/local/bin/index
COPY --from=build /app/idls /app/idls
COPY --from=build /app/public /app/public
WORKDIR /app
ENV DATABASE_URL=sqlite:/data/index.db
EXPOSE 8080
ENTRYPOINT ["index"]
CMD ["run", "--idl-dir", "/app/idls", "--bind", "0.0.0.0:8080", "--db", "sqlite:/data/index.db"]
