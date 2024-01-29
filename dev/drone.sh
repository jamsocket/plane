#!/bin/sh

cargo run -- \
    drone \
    --controller-url ws://localhost:8080/ \
    --cluster "localhost:9090" \
    --db "drone.sqlite" \
    "$@"
