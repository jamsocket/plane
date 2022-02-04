ARG BASE

FROM $BASE as build

COPY . .

RUN cargo build -p spawner-sidecar --release

FROM gcr.io/distroless/cc-debian11

COPY --from=build /app/target/release/spawner-sidecar /spawner-sidecar
ENTRYPOINT [ "/spawner-sidecar" ]
