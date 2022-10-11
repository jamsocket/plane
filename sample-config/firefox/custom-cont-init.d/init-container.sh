#!/bin/sh

set -e

# Obtain the IP of the controller, which serves DNS for the Plane cluster.
DNS_IP=$(dig +short controller)
echo "Controller IP: ${DNS_IP}"

# Write the IP to the dnsmasq config file.
echo "server=/plane.test/${DNS_IP}" >> /etc/dnsmasq.conf

# Prepend localhost DNS server (powered by dnsmasq) to resolv.conf.
echo "nameserver 127.0.0.1" > /tmp/resolv.conf
cat /etc/resolv.conf >> /tmp/resolv.conf
cat /tmp/resolv.conf > /etc/resolv.conf # mv doesn't work but overwriting does.

# Run local DNS server.
dnsmasq
