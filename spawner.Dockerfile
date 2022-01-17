FROM rust:latest as build

WORKDIR /work
COPY . .
RUN cargo test -p spawner --release
RUN cargo build -p spawner --release

FROM gcr.io/distroless/cc-debian11

COPY --from=build /work/target/release/spawner /spawner
ENTRYPOINT [ "/spawner" ]
