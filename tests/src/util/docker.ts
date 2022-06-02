import { DropHandler } from "./environment";
import Dockerode from "dockerode";
import { mkdtempSync, writeFileSync } from "fs";
import { join } from "path";
import { tmpdir } from "os";
import { generateCertificates } from "./certificates.js";
import getPort from "@ava/get-port";

export interface PebbleResult {
  port: number;
  cert: string;
}

export class Docker implements DropHandler {
  docker: Dockerode;
  containers: Array<Dockerode.Container> = [];

  constructor() {
    this.docker = new Dockerode();
  }

  async runNats(): Promise<number> {
    const port = await getPort();
    const imageName = "nats:latest"

    await this.docker.pull(imageName)

    const container = await this.docker.createContainer({
      Image: imageName,
      HostConfig: {
        PortBindings: { ["4222/tcp"]: [{ HostPort: port.toString() }] },
      },
    });

    await container.start();
    this.containers.push(container);

    return port;
  }

  async runPebble(): Promise<PebbleResult> {
    const port = await getPort();
    const tempdir = mkdtempSync(join(tmpdir(), "spawner-pebble-config-"));
    const certs = await generateCertificates();
    const imageName = "letsencrypt/pebble:latest";

    const pebbleConfig = {
      pebble: {
        listenAddress: "0.0.0.0:443",
        managementListenAddress: "0.0.0.0:15000",
        certificate: "/etc/auth/local-cert.pem",
        privateKey: "/etc/auth/local-cert.key",
        httpPort: 5002,
        tlsPort: 5001,
        ocspResponderURL: "",
        externalAccountBindingRequired: false,
      },
    };

    writeFileSync(join(tempdir, "config.json"), JSON.stringify(pebbleConfig));
    await this.docker.pull(imageName)

    const container = await this.docker.createContainer({
      Image: imageName,
      HostConfig: {
        PortBindings: { ["443/tcp"]: [{ HostPort: port.toString() }] },
        Binds: [`${tempdir}:/etc/pebble`, `${certs.parentDir()}:/etc/auth`],
      },
      Env: ["PEBBLE_VA_ALWAYS_VALID=1"],
      ExposedPorts: {
        "443/tcp": {},
      },
      Cmd: ["/usr/bin/pebble", "-config", "/etc/pebble/config.json"],
    });

    await container.start();
    this.containers.push(container);

    return {
      port,
      cert: certs.certificatePath,
    };
  }

  async drop() {
    for (const container of this.containers) {
      try {
        await container.stop();
      } catch (e) {
        console.warn("Problem stopping container.", e);
      }
    }
  }
}
