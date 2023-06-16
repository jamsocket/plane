FROM ghcr.io/drifting-in-space/rust-sccache:latest as build

ARG SCCACHE_BUCKET=""
ARG AWS_SECRET_ACCESS_KEY=""
ARG AWS_ACCESS_KEY_ID=""
ARG AWS_REGION=""

WORKDIR /work

COPY . .

#note: this is safe because build container is transitory
ENV SCCACHE_BUCKET=$SCCACHE_BUCKET AWS_SECRET_ACCESS_KEY=$AWS_SECRET_ACCESS_KEY AWS_ACCESS_KEY_ID=$AWS_ACCESS_KEY_ID SCCACHE_REGION=$AWS_REGION
ENV SCCACHE_C_CUSTOM_CACHE_BUSTER=controller

RUN test $SCCACHE_BUCKET && \
    { RUSTC_WRAPPER=/usr/local/bin/sccache cargo build --bin=plane-controller --release && sccache --show-stats ; } || \
    cargo build --bin=plane-controller --release 

FROM gcr.io/distroless/cc-debian11

COPY --from=build /work/target/release/plane-controller /bin/plane-controller

ENTRYPOINT ["/bin/plane-controller"]
