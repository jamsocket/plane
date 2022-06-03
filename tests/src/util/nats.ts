import { JSONCodec, Msg, Subscription } from "nats"

export const JSON_CODEC = JSONCodec()

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
  