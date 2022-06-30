import { ExecutionContext } from "ava"
import { JSONCodec, Msg, NatsConnection, Subscription } from "nats"

export const JSON_CODEC = JSONCodec()

export async function expectResponse<T, R>(t: ExecutionContext<unknown>, nats: NatsConnection, subject: string, request: T, expectedResponse: R) {
  const responseEnc = await nats.request(subject, JSON_CODEC.encode(request), {timeout: 300})
  const response = JSON_CODEC.decode(responseEnc.data)
  t.deepEqual(response, expectedResponse)
}

export async function expectMessage<T, R>(t: ExecutionContext<unknown>, nats: NatsConnection, subject: string, expected: T, response?: R) {
  const sub = await nats.subscribe(subject, {timeout: 5000})
  const messageEnc = await sub[Symbol.asyncIterator]().next()
  const message = JSON_CODEC.decode(messageEnc.value.data)
  t.deepEqual(message, expected)
  if (typeof response !== "undefined") {
    messageEnc.value.respond(JSON_CODEC.encode(response))
  }
  sub.unsubscribe()
}

export class NatsMessageIterator<T> {
  private iterator: AsyncIterator<Msg, undefined, undefined>

  constructor(sub: Subscription) {
    this.iterator = sub[Symbol.asyncIterator]()
  }

  async next(): Promise<[T, Msg]> {
    const message = await this.iterator.next()
    if (message.value === undefined) {
      throw new Error("Subscription closed when message expected.")
    }

    return [JSON_CODEC.decode(message.value.data) as T, message.value]
  }
}
