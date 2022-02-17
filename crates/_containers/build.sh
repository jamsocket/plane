#!/bin/sh

# Based on:
# https://github.com/GoogleContainerTools/skaffold/blob/main/examples/custom-buildx/buildx.sh

if ! docker buildx inspect skaffold-builder >/dev/null 2>&1; then
  docker buildx create --name skaffold-builder --platform amd64
fi

BASE=$(echo $IMAGE | cut -d":" -f 1)-base

set -x

/usr/bin/docker buildx build \
    --builder skaffold-builder \
    --cache-from type=registry,ref=${BASE}:latest \
    --cache-to type=registry,ref=${BASE}:latest,mode=max \
    --file $1 \
    --tag $IMAGE \
    --load .
