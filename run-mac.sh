#!/bin/bash
xhost + 127.0.0.1
docker compose -f mac-docker-compose.yml
