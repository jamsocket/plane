import { mkdtempSync, rmSync } from "fs"
import { tmpdir } from "os"
import { join } from "path"
import { DroneDatabase } from "./db"
import { DroneRunner } from "./runner"
import { DummyServer } from "./server"
import { sleep } from "./util"

export interface DropHandler {
    drop(): Promise<void>
}

export class TemporaryDirectory implements DropHandler {
    public dir: string

    constructor() {
        this.dir = mkdtempSync(join(tmpdir(), "spawner-test-"))
    }

    async drop() {
        rmSync(this.dir, { recursive: true })
    }
}

export class TestEnvironment implements DropHandler {
    tempdir: TemporaryDirectory
    db: DroneDatabase
    runner: DroneRunner
    dummyServer: DummyServer

    private constructor() {
        this.tempdir = new TemporaryDirectory()
        const dir = this.tempdir.dir

        const dbPath = `${dir}/base.db`
        this.db = new DroneDatabase(dbPath)
        this.runner = new DroneRunner(dbPath)
        this.dummyServer = new DummyServer()
    }

    static async create(): Promise<TestEnvironment> {
        const env = new TestEnvironment()
        await env.runner.migrate()
        return env
    }

    async drop() {
        await this.runner.drop()
        await this.tempdir.drop()
        await this.dummyServer.drop()
    }
}