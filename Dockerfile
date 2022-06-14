FROM rust:buster as build

WORKDIR /work

COPY . .

RUN cargo build --bin=spawner-drone --features=full --release

RUN ls /work/target/release

FROM gcr.io/distroless/cc-debian11

COPY --from=build /work/target/release/spawner-drone /bin/spawner-drone

ENTRYPOINT ["/bin/spawner-drone"]
