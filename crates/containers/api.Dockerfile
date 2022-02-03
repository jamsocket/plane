FROM rust:1.58 AS chef 
RUN cargo install cargo-chef 
WORKDIR app

FROM chef AS plan
COPY . .
RUN cargo chef prepare  --recipe-path recipe.json

FROM chef AS build
COPY --from=plan /app/recipe.json recipe.json

RUN cargo chef cook --release --recipe-path recipe.json

COPY . .

RUN cargo test -p spawner-api --release
RUN cargo build -p spawner-api --release

FROM gcr.io/distroless/cc-debian11

COPY --from=build /app/target/release/spawner-api /spawner-api
ENTRYPOINT [ "/spawner-api" ]
