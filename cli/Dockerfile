FROM rust:buster as build

WORKDIR /work

COPY . .

RUN cargo build --bin=plane-cli --release

FROM gcr.io/distroless/cc-debian11

COPY --from=build /work/target/release/plane-cli /bin/plane-cli

ENTRYPOINT ["/bin/plane-cli"]
