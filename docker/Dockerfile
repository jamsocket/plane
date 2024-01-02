FROM rust:latest as builder

WORKDIR /work

COPY Cargo.toml /work/
COPY Cargo.lock /work/
COPY plane/Cargo.toml /work/plane/Cargo.toml

RUN mkdir /work/plane/src
RUN echo "fn main() {}" > /work/plane/src/main.rs

RUN mkdir -p /work/plane/tests/plane-test-macro/src
COPY plane/tests/plane-test-macro/Cargo.toml /work/plane/tests/plane-test-macro/Cargo.toml
RUN touch /work/plane/tests/plane-test-macro/src/lib.rs

RUN cargo build --release

COPY . .

RUN cargo build --release
 
FROM gcr.io/distroless/cc-debian12
 
COPY --from=builder /work/target/release/plane /bin/plane
 
ENTRYPOINT ["/bin/plane"]
