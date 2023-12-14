#!/bin/sh

cargo run -- dns --controller-url ws://localhost:8080/ "$@"
