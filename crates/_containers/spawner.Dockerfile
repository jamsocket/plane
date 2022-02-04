ARG BASE

FROM $BASE as build

COPY . .

RUN cargo build -p spawner --release

FROM gcr.io/distroless/cc-debian11

COPY --from=build /app/target/release/spawner /spawner
ENTRYPOINT [ "/spawner" ]
