#!/bin/sh

#check that drone id doesn't have periods (for nats)
DRONE_ID=${PLANE_DRONE_ID:-$(curl -s https://api.ipify.org | sed 's/\./_/g')}
NATS_USER=${NATS_USER:-""}
NATS_PASSWORD=${NATS_PASSWORD:-""}
NATS_HOST=${NATS_HOST:-"localhost:4222"}
NATS_URL=${NATS_URL:-"nats://${NATS_USER}:${NATS_PASSWORD}@${NATS_HOST}"}
CLUSTER_NAME=${PLANE_CLUSTER_DOMAIN:-"localhost:8322"}
plane-metrics -n "${NATS_URL}" -d "${DRONE_ID}" -c "${CLUSTER_NAME}"
