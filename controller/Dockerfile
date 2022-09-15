FROM rust:buster as build

WORKDIR /work

COPY . .

RUN cargo build --bin=spawner-controller --release

FROM gcr.io/distroless/cc-debian11

COPY --from=build /work/target/release/spawner-controller /bin/spawner-controller

ENTRYPOINT ["/bin/spawner-controller"]
