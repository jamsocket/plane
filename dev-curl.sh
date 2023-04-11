#!/bin/sh

url=$1
hostname=$(echo "$url" | awk -F[/:] '{print $4}')
path=$(echo "$url" | awk -F[/:] '{for (i=5; i<=NF; i++) printf "/%s", $i}')

curl -H "Host: ${hostname}" https://localhost:4333${path} --insecure -D -
