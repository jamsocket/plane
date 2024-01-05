This creates a “quickstart” container that runs Postgres and Plane (controller, drone, and proxy)
all in one container.

This is not intended for production use, but is useful as a quick way to try out Plane.

Building:

```bash
# in the BASE DIRECTORY of this repo, NOT this directory.
docker build -f docker/quickstart/Dockerfile . -t plane/quickstart
```

```bash
docker run plane/quickstart \
    -p 8080:8080 \
    -p 9090:9090 \
    -v /var/run/docker.sock:/var/run/docker.sock
```

Note: passing `-v /var/run/docker.sock:/var/run/docker.sock` is required to allow the drone to spawn
backends, but it effectively gives the code root access. You should not run this on a machine with a
publicly exposed port 5432, 8080, or 9090.
