FROM rust:latest as builder

WORKDIR /work
COPY plane .
RUN cargo build --release

FROM gcr.io/distroless/cc-debian12
 
COPY --from=builder /work/target/release/plane /bin/plane
ENTRYPOINT ["/bin/plane"]
