---
sidebar_position: 3
---

# Deploying

Plane is meant to be more of an ingredient than a recipe -- the specifics of
deployment will depend on your needs and how you'd like Plane to interact
with the rest of the system.

## NATS

All components in Plane interact over [NATS](https://nats.io/), an open-source
message bus. Plane delegates persistence and authentication to NATS, so you
immediately have all the flexibility that NATS offers out of the box.

The only requirement that Plane imposes on your NATS cluster is that you
enable Jetstream.

For more information on deploying NATS, see their [deployment guide](https://docs.nats.io/running-a-nats-service/introduction).

## DNS

## Certificates

## Drones

### Sandboxing

### Network access
