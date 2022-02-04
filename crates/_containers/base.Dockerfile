FROM lukemathwalker/cargo-chef:latest-rust-1.58 AS chef 
WORKDIR app

FROM chef AS plan
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=plan /app/recipe.json recipe.json

RUN cargo chef cook --release --recipe-path recipe.json
