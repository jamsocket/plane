import { ChildProcess, spawn } from "child_process"
import { mkdtempSync } from "fs"
import { tmpdir } from "os"
import { join } from "path"

const MANIFEST_PATH = process.env.MANIFEST_PATH || "../Cargo.toml"

var nextPort = 10200;
export function assignPort(): number {
    return nextPort++
}

export function makeTempDir(): string {
    return mkdtempSync(join(tmpdir(), "spawner-drone-functional-test-"))
}

export function sleep(durationMillis: number): Promise<void> {
    return new Promise((resolve) => setInterval(resolve, durationMillis))
}

export async function killProcAndWait(proc: ChildProcess): Promise<void> {
    proc.kill("SIGTERM")
    await waitForExit(proc)
}

export function waitForExit(proc: ChildProcess): Promise<void> {
    return new Promise((accept, reject) => {
        const handleResult = () => {
            if (proc.exitCode === 0 || proc.signalCode) {
                accept()
            } else {
                reject(new Error(`Process exited with non-zero code ${proc.exitCode}`))
            }
        }

        proc.on('exit', handleResult)

        if (proc.exitCode !== null) {
            console.log('h0')
            handleResult()
        }
    })
}

export class DroneRunner {
    constructor(private dbPath: string) { }

    static build(): Promise<void> {
        let proc = spawn("cargo", ['build', '--manifest-path', MANIFEST_PATH], {
            stdio: 'inherit'
        })

        return waitForExit(proc)
    }

    async migrate() {
        let proc = spawn("cargo", [
            'run',
            '--manifest-path', MANIFEST_PATH,
            '--',
            "--db-path", this.dbPath
        ], {
            stdio: 'inherit'
        })

        await waitForExit(proc)
    }

    serve(httpPort: number): ChildProcess {
        var args = [
            'run',
            '--manifest-path', MANIFEST_PATH,
            '--',
            "--db-path", this.dbPath
        ]

        if (httpPort !== undefined) {
            args.push("--http-port", httpPort.toString())
        }

        let proc = spawn("cargo", args, {
            stdio: 'inherit'
        })

        proc.on("exit", (code) => {
            if (code !== null) {
                // Server process should not exit until we kill it.
                throw new Error(`Process exited with code ${code}.`)
            }
        })

        return proc
    }
}