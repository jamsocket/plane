import { rejects } from "assert";
import { WebSocket } from "ws";
import { callbackToPromise } from "./promise";

type AcceptRejectPair = [(st: string) => void, (er: Error) => void]

export class WebSocketClient {
    ws: WebSocket
    incomingQueue: Array<string> = []
    promiseQueue: Array<AcceptRejectPair> = []
    closed: boolean = false

    private constructor(url: string, host: string) {
        this.ws = new WebSocket(url, { headers: { host } })
    }

    static async create(url: string, host: string): Promise<WebSocketClient> {
        const client = new WebSocketClient(url, host)

        client.ws.on('message', (msg) => {
            let decodedMsg = msg.toString('utf8')
            if (client.promiseQueue.length > 0) {
                let [accept, _] = client.promiseQueue.shift() as AcceptRejectPair
                accept(decodedMsg)
            } else {
                client.incomingQueue.push(decodedMsg)
            }
        })

        await new Promise((accept, reject) => {
            client.ws.on('open', accept)

            client.ws.on('close', () => {
                client.closed = true
                for (const acceptRejectPair of client.promiseQueue) {
                    let [_, reject] = acceptRejectPair
                    reject(new Error("WebSocket connection closed."))
                }
                
                reject(new Error("WebSocket connection closed before opening."))
            })
    
            client.ws.on('error', (err) => {
                reject(err)
            })
        })

        return client
    }

    close() {
        this.ws.close()
    }

    send(data: string) {
        this.ws.send(data)
    }

    async receive(): Promise<string> {
        if (this.closed) {
            return new Promise((_, reject) => reject(new Error("WebSocket connection is already closed.")))
        }

        if (this.incomingQueue.length > 0) {
            return this.incomingQueue.shift() as string
        }

        return new Promise((accept, reject) => {
            this.promiseQueue.push([accept, reject])
        })
    }
}
