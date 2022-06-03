import anyTest, { TestFn } from "ava"
import { KeyCertPair, validateCertificateKeyPair } from "./util/certificates.js"
import { TestEnvironment } from "./util/environment.js"
import { DroneRunner } from "./util/runner.js"
import { connect } from "nats"
import { sleep } from "./util/sleep.js"
import { mkdirSync } from "fs"
import { JSON_CODEC, NatsMessageIterator } from "./util/nats.js"

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

interface DnsMessage {
  cluster: string
  value: string
}

test("Generate certificate", async (t) => {
  const natsPort = await t.context.docker.runNats()
  const nats = await connect({ port: natsPort })
  const pebble = await t.context.docker.runPebble()

  await sleep(1000)
  mkdirSync(t.context.tempdir.path("keys"))

  const keyPair = new KeyCertPair(
    t.context.tempdir.path("keys/cert.key"),
    t.context.tempdir.path("keys/cert.pem")
  )

  const sub = new NatsMessageIterator<DnsMessage>(
    nats.subscribe("acme.set_dns_record")
  )

  const certRefreshPromise = t.context.runner.certRefresh(
    keyPair,
    natsPort,
    pebble
  )

  const [val, msg] = await sub.next()
  await msg.respond(JSON_CODEC.encode(null))

  t.is(val.cluster, "mydomain.test")
  t.regex(val.value, /^.{10,}$/)

  await certRefreshPromise

  t.assert(validateCertificateKeyPair(keyPair))

  t.pass("oah")
})
