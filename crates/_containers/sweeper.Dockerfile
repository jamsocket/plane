FROM lukemathwalker/cargo-chef:latest-rust-1.58 AS chef

# Prepare chef plan (snapshot dependencies for pre-build).
FROM chef as plan
COPY core /app/core
COPY sweeper /app/sweeper
WORKDIR /app/sweeper
RUN cargo chef prepare --recipe-path recipe.json

# Execute chef plan (pre-build dependencies).
FROM chef AS build
COPY core /app/core
WORKDIR /app/sweeper
COPY --from=plan /app/sweeper/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Build package.
COPY sweeper /app/sweeper
RUN cargo build --release

# Copy binary into image.
FROM gcr.io/distroless/cc-debian11
COPY --from=build /app/sweeper/target/release/spawner-sweeper /spawner-sweeper
ENTRYPOINT [ "/spawner-sweeper" ]
