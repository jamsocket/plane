#!/bin/sh

cargo -q run -- admin --controller http://localhost:8080 "$@"
