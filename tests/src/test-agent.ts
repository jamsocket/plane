import anyTest, { TestFn } from "ava"
import { TestEnvironment } from "./util/environment.js"
import { DroneRunner } from "./util/runner.js"
import { connect } from "nats"
import { sleep } from "./util/sleep.js"
import { JSON_CODEC, NatsMessageIterator } from "./util/nats.js"

/**
 * Path to a world-readable Docker image that serves a "hello world" page.
 * This image is generated from this repo: https://github.com/drifting-in-space/demo-image-hello-world
 */
const HELLO_WORLD_IMAGE =
  "ghcr.io/drifting-in-space/demo-image-hello-world:sha-f98d60d"

const test = anyTest as TestFn<TestEnvironment>

test.before(async () => {
  await DroneRunner.build()
})

test.beforeEach(async (t) => {
  t.context = await TestEnvironment.create()
})

test.afterEach.always(async (t) => {
  await t.context.drop()
})

interface DroneConnectRequest {
  cluster: string
  ip: string
}

interface Duration {
  secs: number
  nanos: number
}

interface SpawnRequest {
  image: string
  backend_id: string
  max_idle_time: Duration
  env: Record<string, string>
  metadata: Record<string, string>
}

type BackendStatus =
  | "Loading"
  | "ErrorLoading"
  | "Starting"
  | "ErrorStarting"
  | "Ready"
  | "TimedOutBeforeReady"
  | "Failed"
  | "Exited"
  | "Swept"

interface BackendStateMessage {
  state: BackendStatus
  time: string
}

test("Spawn with agent", async (t) => {
  const natsPort = await t.context.docker.runNats()
  const nats = await connect({ port: natsPort })
  await sleep(1000)

  const connectionRequestSubscription =
    new NatsMessageIterator<DroneConnectRequest>(
      nats.subscribe("drone.register")
    )

  t.context.runner.runAgent(natsPort)

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
    image: HELLO_WORLD_IMAGE,
    backend_id: "abcde",
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
      nats.subscribe("backend.abcde.status")
    )

  t.is("Loading", (await backendStatusSubscription.next())[0].state)
  t.is("Starting", (await backendStatusSubscription.next())[0].state)
  t.is('Ready', (await backendStatusSubscription.next())[0].state)
  await sleep(500)

  // Result should exist in database.

  const address = await t.context.db.getAddress("abcde")
  t.regex(address, /^127\.0\.0\.1:\d+$/)

  // Result should respond to ping.

  // Status should update to swept.
  // t.is('Swept', (await backendStatusSubscription.next())[0].state)
})
