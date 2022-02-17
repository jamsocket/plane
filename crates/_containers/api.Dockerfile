FROM lukemathwalker/cargo-chef:latest-rust-1.58 AS chef

# Prepare chef plan (snapshot dependencies for pre-build).
FROM chef as plan
COPY . /app
WORKDIR /app
RUN cargo chef prepare --recipe-path recipe.json

# Execute chef plan (pre-build dependencies).
FROM chef AS build
WORKDIR /app
COPY --from=plan /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Build package.
COPY . /app
RUN cargo build --release

# Copy binary into image.
FROM gcr.io/distroless/cc-debian11
COPY --from=build /app/target/release/spawner-api /spawner-api
ENTRYPOINT [ "/spawner-api" ]
