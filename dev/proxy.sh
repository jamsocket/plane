#!/bin/sh

cargo run -- proxy --controller-url ws://localhost:8080/ --cluster "localhost:9090" "$@"
