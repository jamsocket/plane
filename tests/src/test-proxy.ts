import anyTest, { TestFn } from 'ava';
import { DroneDatabase } from "./db"
import { assignPort, DroneRunner, killProcAndWait, makeTempDir, sleep, waitForExit } from "./util"
import { runDevServer } from "./server"
import { Server } from "http"
import { rmSync } from "fs"

interface TestContext {
    //server: Server,
    db: DroneDatabase,
    runner: DroneRunner,
    scratchDir: string,
}

const test = anyTest as TestFn<TestContext>;

test.before(async (t) => {
    await DroneRunner.build()
})

test.beforeEach(async (t) => {
    //t.context.server = runDevServer(assignPort())

    t.context.scratchDir = makeTempDir()
    const dbPath = `${t.context.scratchDir}/base.db`
    t.context.db = new DroneDatabase(dbPath)
    t.context.runner = new DroneRunner(dbPath)

    await t.context.runner.migrate()
})

test.afterEach(async (t) => {
    //t.context.server.close()
    rmSync(t.context.scratchDir, { recursive: true })
})

test("proxy-unrecognized-host", async (t) => {
    const proxyPort = assignPort()
    const proc = t.context.runner.serve(proxyPort)
    //await sleep(5000)

    let result = await fetch(`http://127.0.0.1:${proxyPort}/`, { headers: { 'host': 'foo.bar' } })
    t.is(result.status, 404)

    await killProcAndWait(proc)

    t.pass("okay")
})
