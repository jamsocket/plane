import { WebSocket } from "ws"

type AcceptRejectPair = [(st: string) => void, (er: Error) => void]

export class WebSocketClient {
  ws: WebSocket
  incomingQueue: Array<string> = []
  promiseQueue: Array<AcceptRejectPair> = []
  closed = false

  private constructor(url: string, host: string) {
    this.ws = new WebSocket(url, { headers: { host } })
  }

  static async create(url: string, host: string): Promise<WebSocketClient> {
    const client = new WebSocketClient(url, host)

    client.ws.on("message", (msg) => {
      const decodedMsg = msg.toString("utf8")
      if (client.promiseQueue.length > 0) {
        const [accept] = client.promiseQueue.shift() as AcceptRejectPair
        accept(decodedMsg)
      } else {
        client.incomingQueue.push(decodedMsg)
      }
    })

    await new Promise((accept, reject) => {
      client.ws.on("open", accept)

      client.ws.on("close", () => {
        client.closed = true
        for (const acceptRejectPair of client.promiseQueue) {
          const [, reject] = acceptRejectPair
          reject(new Error("WebSocket connection closed."))
        }

        reject(new Error("WebSocket connection closed before opening."))
      })

      client.ws.on("error", (err) => {
        reject(err)
      })
    })

    return client
  }

  close(): void {
    this.ws.close()
  }

  send(data: string): void {
    this.ws.send(data)
  }

  receive(): Promise<string> {
    if (this.closed) {
      return new Promise((_, reject) =>
        reject(new Error("WebSocket connection is already closed."))
      )
    }

    if (this.incomingQueue.length > 0) {
      return Promise.resolve(this.incomingQueue.shift())
    }

    return new Promise((accept, reject) => {
      this.promiseQueue.push([accept, reject])
    })
  }
}
