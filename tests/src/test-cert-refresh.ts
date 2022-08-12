import { mkdirSync } from "fs"
import { connect } from "nats"
import { KeyCertPair, validateCertificateKeyPair } from "./util/certificates.js"
import { TestEnvironment } from "./util/environment.js"
import { JSON_CODEC, NatsMessageIterator } from "./util/nats.js"
import { sleep } from "./util/sleep.js"
import { DnsMessage } from "./util/types.js"

const test = TestEnvironment.wrappedTestFunction()

test("Generate certificate", async (t) => {
  t.timeout(20000, "Starting NATS")
  const natsPort = await t.context.docker.runNats()
  t.timeout(10000, "Starting Pebble")
  const pebble = await t.context.docker.runPebble()

  await sleep(500)
  t.timeout(5000, "Connecting to NATS")
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

  t.timeout(5000, "Waiting for DNS message.")
  const [val, msg] = await sub.next()
  t.timeout(1000, "Responding to message.")
  await msg.respond(JSON_CODEC.encode(true))

  t.is(val.cluster, "mydomain.test")
  t.regex(val.value, /^.{10,}$/)

  t.timeout(30000, "Waiting for certificate to refresh.")
  await certRefreshPromise

  t.assert(validateCertificateKeyPair(keyPair))
})


test("Generate cert with EAB credentials", async (t) => {
  //t.timeout(20000, "Starting NATS")
  const natsPort = await t.context.docker.runNats()
  //t.timeout(10000, "Starting Pebble")
  const pebble = await t.context.docker.runPebble(true)

  await sleep(500)
  //t.timeout(5000, "Connecting to NATS")
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

  const isEab = true
  const certRefreshPromise = t.context.runner.certRefresh(
    keyPair,
    natsPort,
    pebble,
    isEab,
    { kid: 'kid-1', key: "zWNDZM6eQGHWpSRTPal5eIUYFTu7EajVIoguysqZ9wG44nMEtx3MUAsUDkMTQ12W" }
  )

  t.timeout(5000, "Waiting for DNS message.")
  const [val, msg] = await sub.next()
  t.timeout(1000, "Responding to message")
  await msg.respond(JSON_CODEC.encode(true))

  t.is(val.cluster, "mydomain.test")
  t.regex(val.value, /^.{10,}$/)

  t.timeout(30000, "Waiting for certificate to refresh.")
  await certRefreshPromise
  t.assert(validateCertificateKeyPair(keyPair))
})

test.only("incorrect eab credentials cause panic", async (t) => {
  const natsPort = await t.context.docker.runNats()
  const pebble = await t.context.docker.runPebble(true)
  await sleep(1000)

  mkdirSync(t.context.tempdir.path("keys"))

  const keyPair = new KeyCertPair(
    t.context.tempdir.path("keys/cert.key"),
    t.context.tempdir.path("keys/cert.pem")
  )

  const isEab = true
  t.plan(2)
  try {
    await t.context.runner.certRefresh(
      keyPair,
      natsPort,
      pebble,
      isEab,
      { kid: 'badkid', key: "zWNDZM6eQGHWpSRTPal5eIUYFTu7EajVIoguysqZ9wG44nMEtx3MUAsUDkMTQ12W" }
    )
  } catch (e) {
    t.pass("spawner panics when acme_kid invalid")
  }
  
  try {
    await t.context.runner.certRefresh(
      keyPair,
      natsPort,
      pebble,
      isEab,
      { kid: 'kid-1', key: "badkey" }
    )
  } catch (e) {
    t.pass("spawner panics when acme_key is invalid")
  }
})