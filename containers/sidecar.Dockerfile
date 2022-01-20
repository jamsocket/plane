FROM rust:latest as build

WORKDIR /work
COPY . .
RUN cargo test -p spawner-sidecar --release
RUN cargo build -p spawner-sidecar --release

FROM gcr.io/distroless/cc-debian11

COPY --from=build /work/target/release/spawner-sidecar /spawner-sidecar
ENTRYPOINT [ "/spawner-sidecar" ]
