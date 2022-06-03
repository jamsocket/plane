import { spawn, execSync } from "child_process"
import { mkdirSync, mkdtempSync, readFileSync } from "fs"
import { tmpdir } from "os"
import { dirname, join } from "path"
import { waitForExit } from "./runner.js"

export class KeyCertPair {
  constructor(public privateKeyPath: string, public certificatePath: string) {}

  parentDir(): string {
    return dirname(this.certificatePath)
  }

  getCert(): Buffer {
    return readFileSync(this.certificatePath)
  }
}

export async function generateCertificates(
  keyCertPair?: KeyCertPair
): Promise<KeyCertPair> {
  if (keyCertPair === undefined) {
    const dir = mkdtempSync(join(tmpdir(), "spawner-key-"))
    keyCertPair = new KeyCertPair(
      join(dir, "local-cert.key"),
      join(dir, "local-cert.pem")
    )
  }

  mkdirSync(dirname(keyCertPair.certificatePath), { recursive: true })

  const proc = spawn(
    "openssl",
    [
      "req",
      "-x509",
      "-nodes",
      "-newkey",
      "rsa:2048",
      "-subj",
      "/C=US/ST=New York/L=Brooklyn/O=Drifting in Space Corp./CN=mydomain.test",
      "-addext",
      "subjectAltName = DNS:*.mydomain.test, DNS:localhost",
      "-keyout",
      keyCertPair.privateKeyPath,
      "-out",
      keyCertPair.certificatePath,
    ],
    { stdio: "inherit" }
  )

  await waitForExit(proc)

  return keyCertPair
}

export function getModulus(kind: "rsa" | "x509", path: string): Buffer {
  return execSync(`openssl ${kind} -modulus -noout -in ${path}`)
}

export function validateCertificateKeyPair(keyCertPair: KeyCertPair): boolean {
  const keyModulus = getModulus("rsa", keyCertPair.privateKeyPath)
  const certModulus = getModulus("x509", keyCertPair.certificatePath)

  return keyModulus.compare(certModulus) === 0
}
