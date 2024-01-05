# =======
# Builder
# =======
FROM rust:bookworm as builder

WORKDIR /work
COPY Cargo.toml /work/
COPY Cargo.lock /work/
COPY plane/Cargo.toml /work/plane/Cargo.toml
COPY plane/plane-tests/plane-test-macro/Cargo.toml /work/plane/plane-tests/plane-test-macro/Cargo.toml
COPY plane/plane-tests/Cargo.toml /work/plane/plane-tests/Cargo.toml
COPY plane/plane-dynamic/Cargo.toml /work/plane/plane-dynamic/Cargo.toml

RUN mkdir /work/plane/src
RUN echo "fn main() {}" > /work/plane/src/main.rs

RUN mkdir -p /work/plane/plane-tests/src
RUN mkdir -p /work/plane/plane-tests/plane-test-macro/src
RUN mkdir -p /work/plane/plane-dynamic/src
RUN touch /work/plane/plane-tests/plane-test-macro/src/lib.rs
RUN touch /work/plane/plane-tests/src/lib.rs
RUN touch /work/plane/plane-dynamic/src/lib.rs

RUN cargo build -p plane --release --locked

COPY plane plane
RUN cargo build -p plane --release --locked

# =======
# Runtime
# =======
FROM postgres:bookworm

USER root

RUN apt-get update && \
    apt-get install -y supervisor openssl && \
    rm -rf /var/lib/apt/lists/* && \
    apt-get clean

RUN useradd -ms /bin/bash plane

COPY --from=builder /work/target/release/plane /bin/plane
COPY ./docker/quickstart/supervisord.conf /etc/supervisord.conf

ENTRYPOINT ["supervisord", "-c", "/etc/supervisord.conf"]
