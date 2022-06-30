import { mkdtempSync, rmSync } from "fs"
import { tmpdir } from "os"
import { join } from "path"
import { DroneDatabase } from "./db.js"
import { DroneRunner } from "./runner.js"
import { DummyServer } from "./dummy_server.js"
import { Docker } from "./docker.js"
import anyTest, { TestFn } from "ava"

export interface DropHandler {
  drop(): Promise<void>
}

export class TemporaryDirectory implements DropHandler {
  public dir: string

  constructor() {
    this.dir = mkdtempSync(join(tmpdir(), "spawner-test-"))
  }

  path(path: string): string {
    return join(this.dir, path)
  }

  async drop() {
    rmSync(this.dir, { recursive: true })
  }
}

export class TestEnvironment implements DropHandler {
  private constructor(
    public testTitle: string,
    public db: DroneDatabase,
    public tempdir: TemporaryDirectory,
    public runner: DroneRunner,
    public dummyServer: DummyServer,
    public docker: Docker
  ) {}

  static wrappedTestFunction(): TestFn<TestEnvironment> {
    anyTest.beforeEach(async (t) => {
      t.context = await TestEnvironment.create(t.title)
    })
    
    anyTest.afterEach.always(async (t) => {
      // If something crashes during beforeEach, this is still called.
      // t.context will still have its initial value of {}
      if (t.context instanceof TestEnvironment) {
        await t.context.drop()
      }
    })

    return anyTest
  }

  static async create(title: string): Promise<TestEnvironment> {
    const tempdir = new TemporaryDirectory()
    const dir = tempdir.dir
    const dbPath = `${dir}/base.db`

    const runner = new DroneRunner(dbPath)
    await runner.migrate()
    const dummyServer = new DummyServer()
    const db = await DroneDatabase.create(dbPath)

    const docker = new Docker()
    return new TestEnvironment(title, db, tempdir, runner, dummyServer, docker)
  }

  async drop() {
    await this.runner.drop()
    await this.tempdir.drop()
    await this.dummyServer.drop()
    await this.docker.drop()
  }
}
