#!/bin/sh

set -e

DNS_IP=$(dig +short controller)
echo "Controller IP: ${DNS_IP}"

echo "nameserver ${DNS_IP}" >> /etc/resolv.conf
