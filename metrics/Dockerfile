FROM rust:latest as builder
WORKDIR /usr/src/drone-stats
COPY . .
RUN cargo install --path .

FROM debian:bullseye-slim
RUN apt-get update &&  apt-get install -y curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/cargo/bin/* /usr/local/bin/
COPY run-plane-metrics.sh /usr/local/bin/run-plane-metrics
CMD run-plane-metrics
