import anyTest, { TestFn } from 'ava'
import axios from 'axios'
import { TestEnvironment } from './util/environment'
import { DroneRunner } from './util/runner'
import { generateCertificates } from './util/certificates'
import * as https from 'https'
import { WebSocketClient } from './util/websocket'
import { sleep } from './util/sleep'

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
        { headers: { 'host': 'foobar.mydomain.test' }, validateStatus: () => true })
    t.is(result.status, 200)
    t.is(result.data, "Hello World!")
})

test("Host header is set appropriately", async (t) => {
    let proxy = await t.context.runner.serve()
    let dummyServerPort = await t.context.dummyServer.serveHelloWorld()

    await t.context.db.addProxy("foobar", `127.0.0.1:${dummyServerPort}`)

    let result = await axios.get(`http://127.0.0.1:${proxy.httpPort}/host`,
        { headers: { 'host': 'foobar.mydomain.test' } })

    t.is(result.status, 200)
    t.is(result.data, "foobar.mydomain.test")
})

test("SSL provided at startup works", async (t) => {
    let certs = await generateCertificates()

    let proxy = await t.context.runner.serve(certs)
    let dummyServerPort = await t.context.dummyServer.serveHelloWorld()

    await t.context.db.addProxy("blah", `127.0.0.1:${dummyServerPort}`)

    let result = await axios.get(`https://127.0.0.1:${proxy.httpsPort}/`,
        { headers: { 'host': 'blah.mydomain.test' }, httpsAgent: new https.Agent({ca: certs.getCert()}) })

    t.is(result.status, 200)
    t.is(result.data, "Hello World!")
})

test("WebSockets", async (t) => {
    let wsPort = t.context.dummyServer.serveWebSocket()
    let proxy = await t.context.runner.serve()

    await t.context.db.addProxy("abcd", `127.0.0.1:${wsPort}`)
    let client = await WebSocketClient.create(`ws://127.0.0.1:${proxy.httpPort}`, "abcd.mydomain.test")
    
    client.send("ok")
    t.is("echo: ok", await client.receive())

    client.send("ok2")
    t.is("echo: ok2", await client.receive())

    client.close()    
})

test.todo("Certificate rotation")

test.todo("Multiple subdomains")

test("Connection status information is recorded", async (t) => {
    const {runner, dummyServer, db} = t.context

    let proxy = await runner.serve()
    let dummyServerPort = await dummyServer.serveHelloWorld()

    await db.addProxy("foobar", `127.0.0.1:${dummyServerPort}`)
    await sleep(1000)

    t.is(await db.getLastActiveTime("foobar"), null, "Last active time should initially be null.")

    await axios.get(`http://127.0.0.1:${proxy.httpPort}/`,
        { headers: { 'host': 'foobar.mydomain.test' } })
    
    await sleep(1000)

    const lastActive1 = await db.getLastActiveTime("foobar")
    t.not(lastActive1, null, "After activity, last active time should not be null.")

    await sleep(2000)

    const lastActive2 = await db.getLastActiveTime("foobar") as number
    t.is(lastActive1, lastActive2, "Without activiiy, last active time should not increase.")

    await axios.get(`http://127.0.0.1:${proxy.httpPort}/`,
        { headers: { 'host': 'foobar.mydomain.test' } })
    
    await sleep(1000)

    const lastActive3 = await db.getLastActiveTime("foobar") as number
    t.assert(lastActive3 > lastActive2, "After activity, last active time should increase.")

})
