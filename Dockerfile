FROM rust:latest as build

WORKDIR /work

COPY . .

RUN cargo build --bin=spawner-drone --features=full --release

RUN ls /work/target/release

FROM debian:buster-slim

COPY --from=build /work/target/release/spawner-drone /bin/spawner-drone

ENTRYPOINT ["/bin/spawner-drone"]
