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
  const natsPort = await t.context.docker.runNats()
  const isEab = true
  const pebble = await t.context.docker.runPebble(isEab)

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

  const certRefreshPromise = t.context.runner.certRefresh(
    keyPair,
    natsPort,
    pebble,
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
  const isEab = true
  const pebble = await t.context.docker.runPebble(isEab)
  await sleep(500)
  /* to exercise the certificate code paths, spawner requires a 
     functioning NATS server */
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


  let error_free_cert_refreshes = 0;
  try {
    const certRefreshPromise = t.context.runner.certRefresh(
      keyPair,
      natsPort,
      pebble,
      { kid: 'badkid', key: "zWNDZM6eQGHWpSRTPal5eIUYFTu7EajVIoguysqZ9wG44nMEtx3MUAsUDkMTQ12W" }
    )
    sub.next().then(([_, msg]) => msg.respond(JSON_CODEC.encode(true)))
    await certRefreshPromise
    error_free_cert_refreshes++
  } catch (e) {
    t.true(e instanceof Error, "spawner does not error out when acme_kid is invalid")
  }

  try {
    const certRefreshPromise = t.context.runner.certRefresh(
      keyPair,
      natsPort,
      pebble,
      { kid: 'kid-1', key: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
    )
    sub.next().then(([_, msg]) => msg.respond(JSON_CODEC.encode(true)))
    await certRefreshPromise
    error_free_cert_refreshes++
  } catch (e) {
    //NOTE: check that the errors are ServerErrors in logs, sometimes NATS errors
    //      sneak in. Ideally there'd be a way to check for this here.
    t.true(e instanceof Error, "spawner does not error out when acme_key is invalid")
  }

  t.falsy(error_free_cert_refreshes, "there was an error free cert refresh")
})