import anyTest, { TestFn } from 'ava'
import { KeyCertPair, validateCertificateKeyPair } from './util/certificates.js';
import { TestEnvironment } from './util/environment.js'
import { DroneRunner } from './util/runner.js'
import { connect, JSONCodec, Msg, Subscription } from "nats";
import { sleep } from './util/sleep.js';
import { mkdirSync } from 'fs';

const jsonCodec = JSONCodec();
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

class NatsMessageIterator {
    private iterator: AsyncIterator<Msg, undefined, undefined>

    constructor(sub: Subscription) {
        this.iterator = sub[Symbol.asyncIterator]()
    }

    async next(): Promise<[any, Msg]> {
        let message = await this.iterator.next()
        if (message.value === undefined) {
            throw new Error("Subscription closed when message expected.")
        }

        return [jsonCodec.decode(message.value.data), message.value]
    }
}

test("Generate certificate", async (t) => {
    const natsPort = await t.context.docker.runNats()
    const nats = await connect({port: natsPort})
    const pebble = await t.context.docker.runPebble()

    await sleep(1000)
    mkdirSync(t.context.tempdir.path("keys"))

    const keyPair = new KeyCertPair(
        t.context.tempdir.path("keys/cert.key"),
        t.context.tempdir.path("keys/cert.pem"),
    )

    const sub = new NatsMessageIterator(nats.subscribe("acme.set_dns_record"))
    
    const certRefreshPromise = t.context.runner.certRefresh(keyPair, natsPort, pebble)

    let [val, msg] = await sub.next()
    await msg.respond(jsonCodec.encode(null))
    
    t.is(val.cluster, "mydomain.test")
    t.regex(val.value, /^.{10,}$/)

    await certRefreshPromise

    t.assert(validateCertificateKeyPair(keyPair))

    t.pass("oah")
})
