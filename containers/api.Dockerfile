FROM rust:latest as build

WORKDIR /work
COPY . .
RUN cargo test -p spawner-api --release
RUN cargo build -p spawner-api --release

FROM gcr.io/distroless/cc-debian11

COPY --from=build /work/target/release/spawner-api /spawner-api
ENTRYPOINT [ "/spawner-api" ]
