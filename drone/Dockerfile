FROM rust:1.70.0-buster as build

ARG GIT_HASH
ENV GIT_HASH=${GIT_HASH}

WORKDIR /work

COPY . .

RUN cargo build --bin=plane-drone --release 

FROM gcr.io/distroless/cc-debian11

COPY --from=build /work/target/release/plane-drone /bin/plane-drone

ENTRYPOINT ["/bin/plane-drone"]
