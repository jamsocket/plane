#!/bin/sh

docker run \
    -it \
    --rm \
    --network host \
    postgres:16 \
    psql -h localhost -U postgres
