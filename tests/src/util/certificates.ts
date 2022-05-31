import { spawn } from "child_process";
import { mkdtempSync, readFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import { waitForExit } from "./runner";

export class KeyCertPair {
    constructor(
        public privateKeyPath: string,
        public certificatePath: string) { }

    getCert(): any {
        return readFileSync(this.certificatePath)
    }
}

export async function generateCertificates(keyCertPair?: KeyCertPair): Promise<KeyCertPair> {
    const dir = mkdtempSync(join(tmpdir(), "spawner-key-"));

    if (keyCertPair === undefined) {
        keyCertPair = new KeyCertPair(join(dir, "local-cert.key"), join(dir, "local-cert.pem"))
    }

    let proc = spawn("openssl", [
        "req",
        "-x509",
        "-nodes",
        "-newkey",
        "rsa:2048",
        "-subj",
        "/C=US/ST=New York/L=Brooklyn/O=Drifting in Space Corp./CN=mydomain.test",
        "-addext",
        "subjectAltName = DNS:*.mydomain.test",
        "-keyout",
        keyCertPair.privateKeyPath,
        "-out",
        keyCertPair.certificatePath
    ])

    await waitForExit(proc)
    
    return keyCertPair
}