import anyTest, { TestFn } from "ava"
import axios from "axios"
import { connect } from "nats"
import { TestEnvironment } from "./util/environment.js"
import { generateId } from "./util/id_gen.js"
import { TEST_IMAGE } from "./util/images.js"
import { JSON_CODEC, NatsMessageIterator } from "./util/nats.js"
import { sleep } from "./util/sleep.js"
import { BackendStateMessage, DroneConnectRequest, DroneStatusMessage, SpawnRequest } from "./util/types.js"

const test = anyTest as TestFn<TestEnvironment>

test.beforeEach(async (t) => {
  t.context = await TestEnvironment.create()
})

test.afterEach.always(async (t) => {
  await t.context.drop()
})

test("Test using IP lookup API", async (t) => {
  const natsPort = await t.context.docker.runNats()
  await sleep(100)
  const nats = await connect({ port: natsPort, token: "mytoken" })

  const connectionRequestSubscription =
    new NatsMessageIterator<DroneConnectRequest>(
      nats.subscribe("drone.register")
    )

  const lookupApiPort = await t.context.dummyServer.serveIpAddress()
  t.context.runner.runAgentWithIpApi(natsPort, `http://localhost:${lookupApiPort}/ip`)

  // Initial handshake.
  const [val, msg] = await connectionRequestSubscription.next()
  t.deepEqual(val, {
    cluster: "mydomain.test",
    ip: "21.22.23.24",
  })
})

test("Drone sends ready messages", async (t) => {
  const natsPort = await t.context.docker.runNats()
  await sleep(100)
  const nats = await connect({ port: natsPort, token: "mytoken" })

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
        drone_id: 345,
      },
    })
  )

  const droneStatusSubscription = new NatsMessageIterator<DroneStatusMessage>(
    nats.subscribe("drone.345.status")
  )

  let status1 = (await droneStatusSubscription.next())[0]
  t.is(status1.drone_id, 345)
  t.is(status1.cluster, "mydomain.test")

  let status2 = (await droneStatusSubscription.next())[0]
  t.is(status2.drone_id, 345)
  t.is(status2.cluster, "mydomain.test")
})

test("Spawn with agent", async (t) => {
  const backendId = generateId()

  const natsPort = await t.context.docker.runNats()
  await sleep(100)
  const nats = await connect({ port: natsPort, token: "mytoken" })

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

test("Spawn fails during start", async (t) => {
  const backendId = generateId()

  const natsPort = await t.context.docker.runNats()
  await sleep(100)
  const nats = await connect({ port: natsPort, token: "mytoken" })
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
    image: TEST_IMAGE,
    backend_id: backendId,
    max_idle_time: { secs: 10, nanos: 0 },
    env: {
      PORT: "8080",
      EXIT_CODE: "1",
      EXIT_TIMEOUT: "100",
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
  t.is("ErrorStarting", (await backendStatusSubscription.next())[0].state)
  t.is("ErrorStarting", (await t.context.db.getBackend(backendId)).state)
})

test("Backend fails after ready", async (t) => {
  const backendId = generateId()

  const natsPort = await t.context.docker.runNats()
  await sleep(100)
  const nats = await connect({ port: natsPort, token: "mytoken" })

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

  const address = await t.context.db.getAddress(backendId)
  t.regex(address, /^127\.0\.0\.1:\d+$/)

  // Result should respond to ping.
  await t.throwsAsync(axios.get(`http://${address}/exit/1`))

  t.is("Failed", (await backendStatusSubscription.next())[0].state)
  t.is("Failed", (await t.context.db.getBackend(backendId)).state)
})