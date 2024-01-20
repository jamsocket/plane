#!/bin/sh

PLANE_LOG_JSON=true \
    cargo run -- \
    drone \
    --controller-url ws://localhost:8080/ \
    --cluster "localhost:9090" \
    --db "drone.sqlite" \
    "$@"
