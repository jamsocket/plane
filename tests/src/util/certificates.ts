import { spawn } from "child_process";
import { mkdtemp, mkdtempSync, readFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

export class KeyCertPair {
    constructor(
        public privateKeyPath: string,
        public certificatePath: string) { }

    getCert(): any {
        return readFileSync(this.certificatePath)
    }
}

export function generateCertificates(): Promise<KeyCertPair> {
    const dir = mkdtempSync(join(tmpdir(), "spawner-key-"));
    const privateKeyPath = join(dir, "local-cert.key")
    const certificatePath = join(dir, "local-cert.pem")

    let proc = spawn("openssl", [
        "req",
        "-x509",
        "-nodes",
        "-newkey",
        "rsa:2048",
        "-subj",
        "/C=US/ST=New York/L=Brooklyn/O=Drifting in Space Corp./CN=mydomain.test",
        "-keyout",
        privateKeyPath,
        "-out",
        certificatePath
    ], { stdio: 'inherit' })

    const result = new KeyCertPair(
        privateKeyPath, certificatePath
    )

    return new Promise((accept, reject) => {
        proc.on("exit", (code) => {
            if (code === 0) {
                accept(result)
            } else {
                reject(new Error(`openssl returned code: ${code}`))
            }
        })
    })
}