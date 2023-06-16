FROM rust:1.70.0-buster as build

WORKDIR /work

COPY . .

RUN cargo build --bin=plane-controller --release 

FROM gcr.io/distroless/cc-debian11

COPY --from=build /work/target/release/plane-controller /bin/plane-controller

ENTRYPOINT ["/bin/plane-controller"]
