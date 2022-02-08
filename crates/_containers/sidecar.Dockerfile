FROM lukemathwalker/cargo-chef:latest-rust-1.58 AS chef

# Prepare chef plan (snapshot dependencies for pre-build).
FROM chef as plan
COPY sidecar /app/sidecar
WORKDIR /app/sidecar
RUN cargo chef prepare --recipe-path recipe.json

# Execute chef plan (pre-build dependencies).
FROM chef AS build
WORKDIR /app/sidecar
COPY --from=plan /app/sidecar/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Build package.
COPY sidecar /app/sidecar
RUN cargo build --release

# Copy binary into image.
FROM gcr.io/distroless/cc-debian11
COPY --from=build /app/sidecar/target/release/spawner-sidecar /spawner-sidecar
ENTRYPOINT [ "/spawner-sidecar" ]
