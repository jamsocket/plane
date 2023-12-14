#!/bin/sh

cargo -q run --bin cli -- --controller http://localhost:8080 "$@"
