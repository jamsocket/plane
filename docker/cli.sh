#!/bin/sh

docker run -t --network docker_plane-dev \
    plane/plane admin \
    --controller http://plane-controller:8080 "$@"
