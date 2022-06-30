import axios from "axios"
import { mkdirSync } from "fs"
import * as https from "https"
import { join } from "path"
import { generateCertificates, KeyCertPair } from "./util/certificates.js"
import { TestEnvironment } from "./util/environment.js"
import { sleep } from "./util/sleep.js"
import { WebSocketClient } from "./util/websocket.js"

const test = TestEnvironment.wrappedTestFunction()

test("Unrecognized host returns a 404", async (t) => {
  const proxy = await t.context.runner.runProxy()

  const result = await axios.get(`http://127.0.0.1:${proxy.httpPort}/`, {
    headers: { host: "foo.bar" },
    validateStatus: () => true,
  })

  t.is(result.status, 404)
})

test("Simple request to HTTP server", async (t) => {
  const proxy = await t.context.runner.runProxy()
  const dummyServerPort = await t.context.dummyServer.serveHelloWorld()

  await t.context.db.addProxy(
    "foobar",
    "backend",
    `127.0.0.1:${dummyServerPort}`
  )

  const result = await axios.get(`http://127.0.0.1:${proxy.httpPort}/`, {
    headers: { host: "foobar.mydomain.test" },
    validateStatus: () => true,
  })
  t.is(result.status, 200)
  t.is(result.data, "Hello World!")
})

test("Host header is set appropriately", async (t) => {
  const proxy = await t.context.runner.runProxy()
  const dummyServerPort = await t.context.dummyServer.serveHelloWorld()

  await t.context.db.addProxy(
    "foobar",
    "backend",
    `127.0.0.1:${dummyServerPort}`
  )

  const result = await axios.get(`http://127.0.0.1:${proxy.httpPort}/host`, {
    headers: { host: "foobar.mydomain.test" },
  })

  t.is(result.status, 200)
  t.is(result.data, "foobar.mydomain.test")
})

test("SSL provided at startup works", async (t) => {
  const certs = await generateCertificates()
  const proxy = await t.context.runner.runProxy(certs)
  const dummyServerPort = await t.context.dummyServer.serveHelloWorld()

  await t.context.db.addProxy("blah", "backend", `127.0.0.1:${dummyServerPort}`)

  const result = await axios.get(`https://127.0.0.1:${proxy.httpsPort}/`, {
    headers: { host: "blah.mydomain.test" },
    httpsAgent: new https.Agent({ ca: certs.getCert() }),
  })

  t.is(result.status, 200)
  t.is(result.data, "Hello World!")
})

test("WebSockets", async (t) => {
  const wsPort = await t.context.dummyServer.serveWebSocket()
  const proxy = await t.context.runner.runProxy()

  await t.context.db.addProxy("abcd", "backend", `127.0.0.1:${wsPort}`)
  const client = await WebSocketClient.create(
    `ws://127.0.0.1:${proxy.httpPort}`,
    "abcd.mydomain.test"
  )

  client.send("ok")
  t.is("echo: ok", await client.receive())

  client.send("ok2")
  t.is("echo: ok2", await client.receive())

  client.close()
})

test("Connection status information is recorded", async (t) => {
  const { runner, dummyServer, db } = t.context

  const proxy = await runner.runProxy()
  const dummyServerPort = await dummyServer.serveHelloWorld()

  await db.addProxy("foobar", "backend", `127.0.0.1:${dummyServerPort}`)
  await sleep(1000)

  const lastActive1 = (await db.getLastActiveTime("backend")) as number

  await axios.get(`http://127.0.0.1:${proxy.httpPort}/`, {
    headers: { host: "foobar.mydomain.test" },
  })

  await sleep(2000)

  const lastActive2 = (await db.getLastActiveTime("backend")) as number
  t.assert(
    lastActive2 > lastActive1,
    "After activity, last active time should have increased."
  )

  await sleep(2000)

  const lastActive3 = (await db.getLastActiveTime("backend")) as number
  t.is(
    lastActive3,
    lastActive2,
    "Without activiiy, last active time should not increase."
  )

  await axios.get(`http://127.0.0.1:${proxy.httpPort}/`, {
    headers: { host: "foobar.mydomain.test" },
  })

  await sleep(1000)

  const lastActive4 = (await db.getLastActiveTime("backend")) as number
  t.assert(
    lastActive4 > lastActive3,
    "After activity, last active time should increase."
  )
})

test("Connection status for WebSocket connections", async (t) => {
  const { runner, dummyServer, db } = t.context

  const proxy = await runner.runProxy()
  const wsServerPort = await dummyServer.serveWebSocket()

  await db.addProxy("abcde", "backend", `127.0.0.1:${wsServerPort}`)

  const lastActive1 = (await db.getLastActiveTime("backend")) as number

  const client = await WebSocketClient.create(
    `ws://127.0.0.1:${proxy.httpPort}`,
    "abcde.mydomain.test"
  )
  await sleep(2000)

  const lastActive2 = (await db.getLastActiveTime("backend")) as number
  t.assert(
    lastActive2 > lastActive1,
    "Last active time sould recognize an open WebSocket connection."
  )

  await sleep(2000)
  client.close()
  const lastActive3 = (await db.getLastActiveTime("backend")) as number
  t.assert(
    lastActive3 > lastActive2,
    "Last active time should continue to increase while WebSocket connection is held open."
  )

  await sleep(1000)

  const lastActive4 = (await db.getLastActiveTime("backend")) as number
  t.is(
    lastActive4,
    lastActive3,
    "When connection is closed, last active time should not increase."
  )
})

test("Certificate provided after start-up", async (t) => {
  const certdir = t.context.tempdir.path("proxy-certs")
  mkdirSync(certdir)

  // Start proxy BEFORE generating certificate. Proxy should start without error and
  // wait until certificate is available.
  const certs = new KeyCertPair(
    join(certdir, "proxy-cert.key"),
    join(certdir, "proxy-cert.pem")
  )

  const proxy = await t.context.runner.runProxy(certs)

  const dummyServerPort = await t.context.dummyServer.serveHelloWorld()
  await t.context.db.addProxy("blah", "backend", `127.0.0.1:${dummyServerPort}`)

  await generateCertificates(certs)

  const result = await axios.get(`https://127.0.0.1:${proxy.httpsPort}/`, {
    headers: { host: "blah.mydomain.test" },
    httpsAgent: new https.Agent({ ca: certs.getCert() }),
  })

  t.is(result.status, 200)
  t.is(result.data, "Hello World!")
})

test("Certificate changed while running", async (t) => {
  const certs = await generateCertificates()

  const proxy = await t.context.runner.runProxy(certs)

  const dummyServerPort = await t.context.dummyServer.serveHelloWorld()
  await t.context.db.addProxy("blah", "backend", `127.0.0.1:${dummyServerPort}`)
  const ca1 = certs.getCert()

  const result1 = await axios.get(`https://127.0.0.1:${proxy.httpsPort}/`, {
    headers: { host: "blah.mydomain.test" },
    httpsAgent: new https.Agent({ ca: ca1 }),
  })
  t.is(result1.status, 200)

  await generateCertificates(certs)
  const ca2 = certs.getCert()
  t.not(ca1.toString(), ca2.toString())

  const result2 = await axios.get(`https://127.0.0.1:${proxy.httpsPort}/`, {
    headers: { host: "blah.mydomain.test" },
    httpsAgent: new https.Agent({ ca: ca2 }),
  })

  t.is(result2.status, 200)
})

test.todo("Connection status properly tracks long-lived HTTP connection.")

test.todo("Multiple subdomains")
