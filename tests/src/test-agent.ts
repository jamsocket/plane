import axios from "axios"
import { connect } from "nats"
import { TestEnvironment } from "./util/environment.js"
import { generateId } from "./util/id_gen.js"
import { TEST_IMAGE } from "./util/images.js"
import { expectMessage, expectResponse, NatsMessageIterator } from "./util/nats.js"
import { sleep } from "./util/sleep.js"
import { BackendStateMessage, DroneConnectRequest, DroneStatusMessage, SpawnRequest } from "./util/types.js"

const test = TestEnvironment.wrappedTestFunction()

test("Test using IP lookup API", async (t) => {
  const natsPort = await t.context.docker.runNats()
  await sleep(100)
  const nats = await connect({ port: natsPort, token: "mytoken" })

  const lookupApiPort = await t.context.dummyServer.serveIpAddress()
  t.context.runner.runAgentWithIpApi(natsPort, `http://localhost:${lookupApiPort}/ip`)

  await expectMessage(t, nats, "drone.register", {
    cluster: "mydomain.test",
    ip: "21.22.23.24",
  })
})

test("Drone sends ready messages", async (t) => {
  const natsPort = await t.context.docker.runNats()
  await sleep(100)
  const nats = await connect({ port: natsPort, token: "mytoken" })
  t.context.runner.runAgent(natsPort)

  // Initial handshake.
  await expectMessage(t, nats, "drone.register", {
    cluster: "mydomain.test",
    ip: "123.12.1.123",
  }, {
    Success: {
      drone_id: 345,
    },
  })

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

test("NATS logs", async (t) => {
  const backendId = generateId()

  const natsPort = await t.context.docker.runNats()
  await sleep(100)
  const nats = await connect({ port: natsPort, token: "mytoken" })

  t.context.runner.runAgent(natsPort)

  // Initial handshake.
  await expectMessage(t, nats, "drone.register", {
    cluster: "mydomain.test",
    ip: "123.12.1.123",
  }, {
    Success: {
      drone_id: 1,
    },
  })

  await sleep(100)
  const logSubscription = new NatsMessageIterator<{ fields: Record<string, any> }>(
    nats.subscribe("logs.drone", { timeout: 1000 })
  )

  // Spawn request.
  const request: SpawnRequest = {
    image: TEST_IMAGE,
    backend_id: backendId,
    max_idle_secs: 10,
    env: {
      PORT: "8080",
    },
    metadata: {
      foo: "bar",
    },
  }
  expectResponse(t, nats, "drone.1.spawn", request, true)

  let [result] = await logSubscription.next()
  t.deepEqual(result.fields.metadata, { foo: "bar" })
})

test("Spawn with agent", async (t) => {
  const backendId = generateId()

  const natsPort = await t.context.docker.runNats()
  await sleep(100)
  const nats = await connect({ port: natsPort, token: "mytoken" })

  t.context.runner.runAgent(natsPort)
  await expectMessage(t, nats, "drone.register", {
    cluster: "mydomain.test",
    ip: "123.12.1.123",
  }, {
    Success: {
      drone_id: 1,
    },
  })

  await sleep(100)

  // Spawn request.
  const request: SpawnRequest = {
    image: TEST_IMAGE,
    backend_id: backendId,
    max_idle_secs: 10,
    env: {
      PORT: "8080",
    },
    metadata: {},
  }
  expectResponse(t, nats, "drone.1.spawn", request, true)

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

test("stats are acquired", async (t) => {
  const backendId = generateId()
  const natsPort = await t.context.docker.runNats()
  await sleep(1000)
  const nats = await connect({ port: natsPort, token: "mytoken" })
  t.context.runner.runAgent(natsPort)

  await expectMessage(t, nats, "drone.register", {
    cluster: "mydomain.test",
    ip: "123.12.1.123",
  }, {
    Success: {
      drone_id: 1,
    },
  })

  await sleep(1000)


  // Spawn request.
  const request: SpawnRequest = {
    image: TEST_IMAGE,
    backend_id: backendId,
    max_idle_secs: 40,
    env: {
      PORT: "8080",
    },
    metadata: {},
  }
  expectResponse(t, nats, "drone.1.spawn", request, true)
  await sleep(100)

  const statsStatusSubsription =
    new NatsMessageIterator<unknown>(
      nats.subscribe(`backend.${backendId}.stats`, { timeout: 1000000 })
    )
  const statsMessage = await statsStatusSubsription.next()
  const stats = statsMessage[0]
  t.assert(stats["cpu_use_percent"] > 0 && stats["mem_use_percent"] > 0)
})

test("stats are killed after container dies", async (t) => {
  const backendId = generateId()
  const natsPort = await t.context.docker.runNats()
  await sleep(1000)
  const nats = await connect({ port: natsPort, token: "mytoken" })
  t.context.runner.runAgent(natsPort)

  await expectMessage(t, nats, "drone.register", {
    cluster: "mydomain.test",
    ip: "123.12.1.123"
  }, {
    Success: {
      drone_id: 1,
    }
  })

  await sleep(100)

  const request: SpawnRequest = {
    image: TEST_IMAGE,
    backend_id: backendId,
    max_idle_secs: 10,
    env: {
      PORT: "8080",
    },
    metadata: {},
  }

  expectResponse(t, nats, "drone.1.spawn", request, true)

  await sleep(100)

  const statsStatusSubsription =
    new NatsMessageIterator<unknown>(
      nats.subscribe(`backend.${backendId}.stats`, { timeout: 1000000 })
    )

  while (true) {
    //NOTE: this sleep is strongly coupled to 
    //DEFAULT_DOCKER_THROTTLED_STATS_INTERVAL_SEC in agent/docker.rs
    let newMessage = await Promise.race([statsStatusSubsription.next(), sleep(11000)])
    if (!newMessage) {
      break;
    } else {
      console.log(newMessage[0])
    }
  }
  t.pass("Eventually, stats should stop coming")
})


test("Lifecycle is managed when agent is restarted.", async (t) => {
  const backendId = generateId()

  const natsPort = await t.context.docker.runNats()
  await sleep(100)
  const nats = await connect({ port: natsPort, token: "mytoken" })

  t.context.runner.runAgent(natsPort)
  await expectMessage(t, nats, "drone.register", {
    cluster: "mydomain.test",
    ip: "123.12.1.123",
  }, {
    Success: {
      drone_id: 1,
    },
  })

  await sleep(100)

  // Spawn request.
  const request: SpawnRequest = {
    image: TEST_IMAGE,
    backend_id: backendId,
    max_idle_secs: 10,
    env: {
      PORT: "8080",
    },
    metadata: {},
  }
  expectResponse(t, nats, "drone.1.spawn", request, true)

  // Status update stages
  const backendStatusSubscription =
    new NatsMessageIterator<BackendStateMessage>(
      nats.subscribe(`backend.${backendId}.status`)
    )

  t.is("Loading", (await backendStatusSubscription.next())[0].state)
  t.is("Starting", (await backendStatusSubscription.next())[0].state)
  t.is("Ready", (await backendStatusSubscription.next())[0].state)

  // Restart drone.
  t.context.runner.drop()
  t.context.runner.runAgent(natsPort)
  t.timeout(5000, "Failed while waiting for drone register request.")
  await expectMessage(t, nats, "drone.register", {
    cluster: "mydomain.test",
    ip: "123.12.1.123",
  }, {
    Success: {
      drone_id: 1,
    },
  })

  // Status should update to swept after ~10 seconds.
  t.timeout(10000, "Failed while waiting for backend to be swept.")
  t.is("Swept", (await backendStatusSubscription.next())[0].state)
  t.is("Swept", (await t.context.db.getBackend(backendId)).state)
})

test("Spawn fails during start", async (t) => {
  const backendId = generateId()

  const natsPort = await t.context.docker.runNats()
  await sleep(100)
  t.timeout(100, "Connecting to NATS.")
  const nats = await connect({ port: natsPort, token: "mytoken" })

  t.context.runner.runAgent(natsPort)

  t.timeout(1000, "Waiting for drone registration message.")
  await expectMessage(t, nats, "drone.register", {
    cluster: "mydomain.test",
    ip: "123.12.1.123",
  }, {
    Success: {
      drone_id: 1,
    },
  })

  await sleep(100)

  // Spawn request.
  const request: SpawnRequest = {
    image: TEST_IMAGE,
    backend_id: backendId,
    max_idle_secs: 10,
    env: {
      PORT: "8080",
      EXIT_CODE: "1",
      EXIT_TIMEOUT: "100",
    },
    metadata: {},
  }
  expectResponse(t, nats, "drone.1.spawn", request, true)

  // Status update stages
  const backendStatusSubscription =
    new NatsMessageIterator<BackendStateMessage>(
      nats.subscribe(`backend.${backendId}.status`)
    )

  t.timeout(100, "Waiting for loading message")
  t.is("Loading", (await backendStatusSubscription.next())[0].state)
  t.timeout(10000, "Waiting for starting message")
  t.is("Starting", (await backendStatusSubscription.next())[0].state)
  t.timeout(5000, "Waiting for ErrorStarting message")
  t.is("ErrorStarting", (await backendStatusSubscription.next())[0].state)
  t.timeout(100, "Waiting for ErrorStarting status response")
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
  await expectMessage(t, nats, "drone.register", {
    cluster: "mydomain.test",
    ip: "123.12.1.123",
  }, {
    Success: {
      drone_id: 1,
    },
  })

  await sleep(100)

  // Spawn request.
  const request: SpawnRequest = {
    image: TEST_IMAGE,
    backend_id: backendId,
    max_idle_secs: 10,
    env: {
      PORT: "8080",
    },
    metadata: {},
  }
  expectResponse(t, nats, "drone.1.spawn", request, true)

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
