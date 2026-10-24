# syntax=docker/dockerfile:1
FROM rust:1-trixie AS chef
WORKDIR /app
COPY rust-toolchain.toml .
RUN cargo install cargo-chef --locked

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked

FROM debian:trixie-slim
RUN useradd --system --uid 10001 --no-create-home wire
COPY --from=builder /app/target/release/wire /usr/local/bin/wire
USER 10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/wire"]

