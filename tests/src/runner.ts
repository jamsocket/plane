import { ChildProcess, spawn } from "child_process"
import { DropHandler } from "./environment";
import { assignPort, sleep } from "./util";

const MANIFEST_PATH = process.env.MANIFEST_PATH || "../Cargo.toml"

export async function killProcAndWait(proc: ChildProcess): Promise<void> {
    proc.kill("SIGTERM")
    await waitForExit(proc)
}

export function waitForExit(proc: ChildProcess): Promise<void> {
    return new Promise((accept, reject) => {
        proc.on('exit', () => {
            if (proc.exitCode === 0 || proc.signalCode) {
                accept()
            } else {
                reject(new Error(`Process exited with non-zero code ${proc.exitCode}`))
            }
        })
    })
}

export class DroneRunner implements DropHandler {
    server?: ChildProcess

    constructor(private dbPath: string) { }

    async drop() {
        if (this.server !== undefined) {
            console.log("Waiting for server to exit.")
            await killProcAndWait(this.server)
        }
    }

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

    async serve(): Promise<number> {
        const httpPort = assignPort()

        var args = [
            'run',
            '--manifest-path', MANIFEST_PATH,
            '--',
            "--db-path", this.dbPath,
            "--http-port", httpPort.toString()
        ]

        let proc = spawn("cargo", args, {
            stdio: 'inherit'
        })

        proc.on("exit", (code) => {
            if (code !== null) {
                // Server process should not exit until we kill it.
                throw new Error(`Process exited with code ${code}.`)
            }
        })

        this.server = proc
        await sleep(500)

        return httpPort
    }
}