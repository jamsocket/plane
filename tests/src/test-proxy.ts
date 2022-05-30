import anyTest, { TestFn } from 'ava'
import axios from 'axios'
import { TestEnvironment } from './util/environment'
import { DroneRunner } from './util/runner'
import { generateCertificates } from './util/certificates'
import * as https from 'https'
import { WebSocketClient } from './util/websocket'

const test = anyTest as TestFn<TestEnvironment>;

test.before(async (t) => {
    await DroneRunner.build()
})

test.beforeEach(async (t) => {
    t.context = await TestEnvironment.create()
})

test.afterEach.always(async (t) => {
    await t.context.drop()
})

test("Unrecognized host returns a 404", async (t) => {
    let proxy = await t.context.runner.serve()

    let result = await axios.get(`http://127.0.0.1:${proxy.httpPort}/`,
        { headers: { 'host': 'foo.bar' }, validateStatus: () => true })

    t.is(result.status, 404)
})

test("Simple request to HTTP server", async (t) => {
    let proxy = await t.context.runner.serve()
    let dummyServerPort = await t.context.dummyServer.serveHelloWorld()

    await t.context.db.addProxy("foobar", `127.0.0.1:${dummyServerPort}`)

    let result = await axios.get(`http://127.0.0.1:${proxy.httpPort}/`,
        { headers: { 'host': 'foobar' }, validateStatus: () => true })
    t.is(result.status, 200)
    t.is(result.data, "Hello World!")
})

test("Host header is set appropriately", async (t) => {
    let proxy = await t.context.runner.serve()
    let dummyServerPort = await t.context.dummyServer.serveHelloWorld()

    await t.context.db.addProxy("foobar", `127.0.0.1:${dummyServerPort}`)

    let result = await axios.get(`http://127.0.0.1:${proxy.httpPort}/host`,
        { headers: { 'host': 'foobar' } })

    t.is(result.status, 200)
    t.is(result.data, "foobar")
})

test("SSL provided at startup works", async (t) => {
    let certs = await generateCertificates()

    let proxy = await t.context.runner.serve(certs)
    let dummyServerPort = await t.context.dummyServer.serveHelloWorld()

    await t.context.db.addProxy("mydomain.test", `127.0.0.1:${dummyServerPort}`)

    let result = await axios.get(`https://127.0.0.1:${proxy.httpsPort}/`,
        { headers: { 'host': 'mydomain.test' }, httpsAgent: new https.Agent({ca: certs.getCert()}) })

    t.is(result.status, 200)
    t.is(result.data, "Hello World!")
})

test.failing("WebSockets", async (t) => {
    let wsPort = t.context.dummyServer.serveWebSocket()
    let proxy = await t.context.runner.serve()

    await t.context.db.addProxy("mydomain.test", `127.0.0.1:${wsPort}`)
    let client = await WebSocketClient.create(`ws://127.0.0.1:${proxy.httpPort}`, "mydomain.test")
    
    client.send("ok")
    t.is("echo: ok", await client.receive())

    client.send("ok2")
    t.is("echo: ok2", await client.receive())

    client.close()    
})

test.todo("Certificate rotation")

test.todo("Multiple subdomains")

test.todo("Connection status information is recorded")

