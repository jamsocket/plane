import { ChildProcess, spawn } from "child_process"
import { DropHandler } from "./environment.js"
import { KeyCertPair, EabOptions } from "./certificates.js"
import { sleep } from "./sleep.js"
import { PebbleResult } from "./docker.js"
import getPort from "@ava/get-port"

const SPAWNER_PATH = "../target/debug/spawner-drone"
const CLUSTER_DOMAIN = "mydomain.test"

export function waitForExit(proc: ChildProcess): Promise<void> {
  return new Promise((accept, reject) => {
    proc.on("exit", () => {
      if (proc.exitCode === 0 || proc.signalCode) {
        accept()
      } else {
        reject(new Error(`Process exited with non-zero code ${proc.exitCode}`))
      }
    })
  })
}

export async function killProcAndWait(proc: ChildProcess): Promise<void> {
  proc.kill("SIGTERM")
  await waitForExit(proc)
}

export interface ServeResult {
  httpPort: number
  httpsPort?: number
}

export class DroneRunner implements DropHandler {
  server?: ChildProcess

  constructor(private dbPath: string) { }

  async drop() {
    if (this.server !== undefined) {
      await killProcAndWait(this.server)
    }
  }

  async migrate() {
    const proc = spawn(
      SPAWNER_PATH,
      ["--db-path", this.dbPath, "--cluster-domain", CLUSTER_DOMAIN, "migrate"],
      {
        stdio: "inherit",
      }
    )

    await waitForExit(proc)
  }

  async certRefresh(
    certs: KeyCertPair,
    natsPort: number,
    pebble: PebbleResult,
    eabOptions?: EabOptions
  ) {
    const eab_cli_options = eabOptions ? [
      "--acme-eab-key",
      eabOptions?.key,
      "--acme-eab-kid",
      eabOptions?.kid,
    ] : []
    const proc = spawn(
      SPAWNER_PATH,
      [
        "--nats-url",
        `nats://mytoken@localhost:${natsPort}`,
        "--https-private-key",
        certs.privateKeyPath,
        "--https-certificate",
        certs.certificatePath,
        "--cluster-domain",
        CLUSTER_DOMAIN,
        "--cert-email",
        "test@test.com",
        "--acme-server",
        `https://localhost:${pebble.port}/dir`,
        ...eab_cli_options,
        "cert",
      ],
      {
        stdio: "inherit",
        env: {
          SPAWNER_TEST_ALLOWED_CERTIFICATE: pebble.cert
        },
      }
    )

    await waitForExit(proc)
  }

  runAgentWithIpApi(natsPort: number, ipApi: string) {
    const args = [
      "--cluster-domain",
      CLUSTER_DOMAIN,
      "--db-path",
      this.dbPath,
      "--nats-url",
      `nats://mytoken@localhost:${natsPort}`,
      "--ip-api",
      ipApi,
      "--host-ip",
      "127.0.0.1",
      "serve",
      "--agent",
    ]

    const proc = spawn(SPAWNER_PATH, args, {
      stdio: "inherit",
    })

    proc.on("exit", (code) => {
      if (code !== null) {
        // Server process should not exit until we kill it.
        throw new Error(`Process exited with code ${code}.`)
      }
    })

    this.server = proc
  }

  runAgent(natsPort: number) {
    const args = [
      "--cluster-domain",
      CLUSTER_DOMAIN,
      "--db-path",
      this.dbPath,
      "--nats-url",
      `nats://mytoken@localhost:${natsPort}`,
      "--ip",
      "123.12.1.123",
      "--host-ip",
      "127.0.0.1",
      "serve",
      "--agent",
    ]

    const proc = spawn(SPAWNER_PATH, args, {
      stdio: "inherit",
    })

    proc.on("exit", (code) => {
      if (code !== null) {
        // Server process should not exit until we kill it.
        throw new Error(`Process exited with code ${code}.`)
      }
    })

    this.server = proc
  }

  async runEverything(natsPort: number, certs: KeyCertPair, pebble: PebbleResult): Promise<void> {
    const httpPort = await getPort()
    const httpsPort = await getPort()

    const args = [
      "--cluster-domain",
      CLUSTER_DOMAIN,
      "--db-path",
      this.dbPath,
      "--nats-url",
      `nats://mytoken@localhost:${natsPort}`,
      "--ip",
      "123.12.1.123",
      "--host-ip",
      "127.0.0.1",
      "--http-port",
      httpPort.toString(),
      "--https-port",
      httpsPort.toString(),
      "--https-private-key",
      certs.privateKeyPath,
      "--https-certificate",
      certs.certificatePath,
      "--cert-email",
      "test@test.com",
      "--acme-server",
      `https://localhost:${pebble.port}/dir`,
    ]

    const proc = spawn(SPAWNER_PATH, args, {
      stdio: "inherit",
      env: {
        SPAWNER_TEST_ALLOWED_CERTIFICATE: pebble.cert,
      },
    })

    proc.on("exit", (code) => {
      if (code !== null) {
        // Server process should not exit until we kill it.
        throw new Error(`Process exited with code ${code}.`)
      }
    })

    this.server = proc
  }

  async runProxy(certs?: KeyCertPair): Promise<ServeResult> {
    const httpPort = await getPort()
    let httpsPort

    const args = [
      "--cluster-domain",
      CLUSTER_DOMAIN,
      "--db-path",
      this.dbPath,
      "--http-port",
      httpPort.toString(),
    ]

    if (certs !== undefined) {
      httpsPort = await getPort()

      args.push(
        "--https-port",
        httpsPort.toString(),
        "--https-private-key",
        certs.privateKeyPath,
        "--https-certificate",
        certs.certificatePath
      )
    }

    args.push("serve", "--proxy")

    const proc = spawn(SPAWNER_PATH, args, {
      stdio: "inherit",
    })

    proc.on("exit", (code) => {
      if (code !== null) {
        // Server process should not exit until we kill it.
        throw new Error(`Process exited with code ${code}.`)
      }
    })

    this.server = proc
    await sleep(500)

    return { httpPort, httpsPort }
  }
}

export async function waitURL(url: URL, interval = 10) {
  await sleep(interval).then(() => fetch(url).catch(async (e) => e.cause.code !== "ECONNRESET" ? true : waitURL(url)))
}
