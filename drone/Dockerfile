FROM rust:latest as build

WORKDIR /work

COPY . .

RUN apt-get update && apt-get install --yes libpq-dev wget

#note: this is safe because build container is transitory
ARG SCCACHE_BUCKET
ARG AWS_SECRET_ACCESS_KEY
ARG AWS_ACCESS_KEY_ID 
ENV SCCACHE_BUCKET=$SCCACHE_BUCKET AWS_SECRET_ACCESS_KEY=$AWS_SECRET_ACCESS_KEY AWS_ACCESS_KEY_ID=$AWS_ACCESS_KEY_ID

#add scache to speed up builds
RUN wget https://github.com/mozilla/sccache/releases/download/v0.3.0/sccache-v0.3.0-x86_64-unknown-linux-musl.tar.gz \
    && tar xzf sccache-v0.3.0-x86_64-unknown-linux-musl.tar.gz \
    && mv sccache-v0.3.0-x86_64-unknown-linux-musl/sccache /usr/local/bin/sccache \
    && chmod +x /usr/local/bin/sccache

RUN RUSTC_WRAPPER=/usr/local/bin/sccache cargo build --bin=plane-drone --release && sccache --show-stats

FROM gcr.io/distroless/cc-debian11

COPY --from=build /work/target/release/plane-drone /bin/plane-drone

ENTRYPOINT ["/bin/plane-drone"]
