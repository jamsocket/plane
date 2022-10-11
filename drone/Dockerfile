FROM rust:buster as build

WORKDIR /work

COPY . .

RUN cargo build --bin=plane-drone --release

FROM gcr.io/distroless/cc-debian11

COPY --from=build /work/target/release/plane-drone /bin/plane-drone

ENTRYPOINT ["/bin/plane-drone"]
