FROM rust:latest as builder

WORKDIR /work

COPY Cargo.toml /work/
COPY Cargo.lock /work/
COPY plane2/Cargo.toml /work/plane2/Cargo.toml

RUN mkdir /work/plane2/src
RUN echo "fn main() {}" > /work/plane2/src/main.rs

RUN mkdir -p /work/plane2/tests/plane-test-macro/src
COPY plane2/tests/plane-test-macro/Cargo.toml /work/plane2/tests/plane-test-macro/Cargo.toml
RUN touch /work/plane2/tests/plane-test-macro/src/lib.rs

RUN cargo build --release

COPY . .

RUN cargo build --release
 
FROM gcr.io/distroless/cc-debian12
 
COPY --from=builder /work/target/release/plane2 /bin/plane2
 
ENTRYPOINT ["/bin/plane2"]
