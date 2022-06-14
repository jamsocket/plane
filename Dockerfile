FROM rust:latest as build

WORKDIR /work

COPY . .

RUN cargo build --bin=spawner-drone --features=full --release

RUN ls /work/target/release

FROM alpine:latest

COPY --from=build /work/target/release/spawner-drone /bin/spawner-drone

CMD ["/bin/spawner-drone"]
