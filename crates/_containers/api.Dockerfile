FROM lukemathwalker/cargo-chef:latest-rust-1.58 AS chef

# Prepare chef plan (snapshot dependencies for pre-build).
FROM chef as plan
COPY core /app/core
COPY api /app/api
WORKDIR /app/api
RUN cargo chef prepare --recipe-path recipe.json

# Execute chef plan (pre-build dependencies).
FROM chef AS build
COPY core /app/core
WORKDIR /app/api
COPY --from=plan /app/api/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Build package.
COPY api /app/api
RUN cargo build --release

# Copy binary into image.
FROM gcr.io/distroless/cc-debian11
COPY --from=build /app/api/target/release/spawner-api /spawner-api
ENTRYPOINT [ "/spawner-api" ]
