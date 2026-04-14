FROM rust:bookworm AS chef

RUN cargo install cargo-chef --locked

RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    clang \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release --bin price-oracle

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd --system oracle && useradd --system --gid oracle oracle

COPY --from=builder /app/target/release/price-oracle /usr/local/bin/price-oracle

USER oracle

ENTRYPOINT ["price-oracle"]
CMD ["start", "--config", "/etc/price-oracle/config.toml"]
