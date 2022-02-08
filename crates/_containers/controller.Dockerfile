FROM lukemathwalker/cargo-chef:latest-rust-1.58 AS chef

# Prepare chef plan (snapshot dependencies for pre-build).
FROM chef as plan
COPY resource /app/resource
COPY controller /app/controller
WORKDIR /app/controller
RUN cargo chef prepare --recipe-path recipe.json

# Execute chef plan (pre-build dependencies).
FROM chef AS build
COPY resource /app/resource
WORKDIR /app/controller
COPY --from=plan /app/controller/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Build package.
COPY controller /app/controller
RUN cargo build --release

# Copy binary into image.
FROM gcr.io/distroless/cc-debian11
COPY --from=build /app/controller/target/release/spawner-controller /spawner-controller
ENTRYPOINT [ "/spawner-controller" ]
