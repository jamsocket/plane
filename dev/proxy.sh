#!/bin/sh

PLANE_LOG_JSON=true \
    cargo run -- \
    proxy \
    --controller-url ws://localhost:8080/ \
    --cluster "localhost:9090" \
    "$@"
