FROM rust:latest

WORKDIR /work

COPY . .

RUN cargo build --release

ENTRYPOINT [ "/work/target/release/kube-play" ]
