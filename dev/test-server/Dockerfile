FROM rust:buster as build

WORKDIR /work

COPY . .

RUN cargo build --release

RUN ls /work/target/release

FROM gcr.io/distroless/cc-debian11

COPY --from=build /work/target/release/test-server /bin/test-server

ENTRYPOINT ["/bin/test-server"]