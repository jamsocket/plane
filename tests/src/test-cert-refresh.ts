import anyTest, { TestFn } from "ava"
import { mkdirSync } from "fs"
import { connect } from "nats"
import { KeyCertPair, validateCertificateKeyPair } from "./util/certificates.js"
import { TestEnvironment } from "./util/environment.js"
import { JSON_CODEC, NatsMessageIterator } from "./util/nats.js"
import { sleep } from "./util/sleep.js"
import { DnsMessage } from "./util/types.js"

const test = anyTest as TestFn<TestEnvironment>

test.beforeEach(async (t) => {
  t.context = await TestEnvironment.create()
})

test.afterEach.always(async (t) => {
  await t.context.drop()
})

test("Generate certificate", async (t) => {
  const natsPort = await t.context.docker.runNats()
  const pebble = await t.context.docker.runPebble()

  await sleep(500)
  const nats = await connect({ port: natsPort, token: "mytoken" })
  await sleep(500)
  
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
  await msg.respond(JSON_CODEC.encode(true))

  t.is(val.cluster, "mydomain.test")
  t.regex(val.value, /^.{10,}$/)

  await certRefreshPromise

  t.assert(validateCertificateKeyPair(keyPair))

  t.pass("oah")
})
