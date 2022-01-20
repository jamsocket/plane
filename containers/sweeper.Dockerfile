FROM rust:latest as build

WORKDIR /work
COPY . .
RUN cargo test -p spawner-sweeper --release
RUN cargo build -p spawner-sweeper --release

FROM gcr.io/distroless/cc-debian11

COPY --from=build /work/target/release/spawner-sweeper /spawner-sweeper
ENTRYPOINT [ "/spawner-sweeper" ]
