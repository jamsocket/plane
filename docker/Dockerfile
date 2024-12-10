FROM rust:1.83-bookworm AS builder

WORKDIR /work
COPY Cargo.toml /work/
COPY Cargo.lock /work/
COPY dynamic-proxy/Cargo.toml /work/dynamic-proxy/Cargo.toml
COPY plane/Cargo.toml /work/plane/Cargo.toml
COPY plane/plane-tests/plane-test-macro/Cargo.toml /work/plane/plane-tests/plane-test-macro/Cargo.toml
COPY plane/plane-tests/Cargo.toml /work/plane/plane-tests/Cargo.toml
COPY common/Cargo.toml /work/common/Cargo.toml

RUN mkdir /work/plane/src
RUN echo "fn main() {}" > /work/plane/src/main.rs

RUN mkdir -p /work/dynamic-proxy/src
RUN touch /work/dynamic-proxy/src/lib.rs

RUN mkdir -p /work/plane/plane-tests/src
RUN mkdir -p /work/plane/plane-tests/plane-test-macro/src
RUN touch /work/plane/plane-tests/plane-test-macro/src/lib.rs
RUN touch /work/plane/plane-tests/src/lib.rs
RUN mkdir -p /work/common/src
RUN touch /work/common/src/lib.rs

RUN cargo build -p plane-dynamic-proxy --release --locked
RUN cargo build -p plane-common --release --locked
RUN cargo build -p plane --release --locked

COPY .cargo .cargo
COPY .git .git
COPY plane plane
COPY dynamic-proxy dynamic-proxy
COPY common common
RUN cargo build -p plane --release --locked

FROM gcr.io/distroless/cc-debian12
 
COPY --from=builder /work/target/release/plane /bin/plane
ENTRYPOINT ["/bin/plane"]
