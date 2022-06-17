import anyTest, { TestFn } from "ava"
import axios from "axios"
import { mkdirSync } from "fs"
import { connect } from "nats"
import { KeyCertPair } from "./util/certificates.js"
import { TestEnvironment } from "./util/environment.js"
import { generateId } from "./util/id_gen.js"
import { TEST_IMAGE } from "./util/images.js"
import { JSON_CODEC, NatsMessageIterator } from "./util/nats.js"
import { sleep } from "./util/sleep.js"
import { BackendStateMessage, DnsMessage, DroneConnectRequest, SpawnRequest } from "./util/types.js"

const test = anyTest as TestFn<TestEnvironment>

test.beforeEach(async (t) => {
    t.context = await TestEnvironment.create()
})

test.afterEach.always(async (t) => {
    await t.context.drop()
})

test("Test end-to-end", async (t) => {
    const backendId = generateId()

    const pebble = await t.context.docker.runPebble()
    const natsPort = await t.context.docker.runNats()
    await sleep(100)
    const nats = await connect({ port: natsPort, token: "mytoken" })

    const connectionRequestSubscription =
        new NatsMessageIterator<DroneConnectRequest>(
            nats.subscribe("drone.register")
        )

    mkdirSync(t.context.tempdir.path("keys"))
    const keyPair = new KeyCertPair(
        t.context.tempdir.path("keys/cert.key"),
        t.context.tempdir.path("keys/cert.pem")
    )

    const sub = new NatsMessageIterator<DnsMessage>(
        nats.subscribe("acme.set_dns_record")
    )

    await t.context.runner.runEverything(natsPort, keyPair, pebble)

    const [, msg1] = await sub.next()
    await msg1.respond(JSON_CODEC.encode(true))

    // Initial handshake.
    const [val, msg] = await connectionRequestSubscription.next()
    t.deepEqual(val, {
        cluster: "mydomain.test",
        ip: "123.12.1.123",
    })

    await msg.respond(
        JSON_CODEC.encode({
            Success: {
                drone_id: 1,
            },
        })
    )

    await sleep(100)

    // Spawn request.
    const request: SpawnRequest = {
        image: TEST_IMAGE,
        backend_id: backendId,
        max_idle_time: { secs: 10, nanos: 0 },
        env: {
            PORT: "8080",
        },
        metadata: {},
    }
    const rawSpawnResult = await nats.request(
        "drone.1.spawn",
        JSON_CODEC.encode(request)
    )
    const spawnResult = JSON_CODEC.decode(rawSpawnResult.data)

    t.is(spawnResult, true)

    // Status update stages
    const backendStatusSubscription =
        new NatsMessageIterator<BackendStateMessage>(
            nats.subscribe(`backend.${backendId}.status`)
        )

    t.is("Loading", (await backendStatusSubscription.next())[0].state)
    t.is("Starting", (await backendStatusSubscription.next())[0].state)
    t.is("Ready", (await backendStatusSubscription.next())[0].state)

    t.is("Ready", (await t.context.db.getBackend(backendId)).state)

    await sleep(500)

    // Result should exist in database.

    const address = await t.context.db.getAddress(backendId)
    t.regex(address, /^127\.0\.0\.1:\d+$/)

    // Result should respond to ping.
    const result = await axios.get(`http://${address}`)
    t.is(result.status, 200)
    t.is(result.data, "Hello World!")

    // Status should update to swept after ~10 seconds.
    t.is("Swept", (await backendStatusSubscription.next())[0].state)
    t.is("Swept", (await t.context.db.getBackend(backendId)).state)
})
