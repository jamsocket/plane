FROM rust:latest as build

WORKDIR /work
COPY . .
RUN cargo build --release

FROM gcr.io/distroless/cc-debian11

COPY --from=build /work/target/release/spawner /spawner
ENTRYPOINT [ "/spawner" ]
