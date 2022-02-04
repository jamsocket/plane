ARG BASE

FROM $BASE as build

COPY . .

RUN cargo build -p spawner-sweeper --release

FROM gcr.io/distroless/cc-debian11

COPY --from=build /app/target/release/spawner-sweeper /spawner-sweeper
ENTRYPOINT [ "/spawner-sweeper" ]
