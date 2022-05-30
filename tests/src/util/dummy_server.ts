import * as express from "express"
import { Server } from "http"
import { WebSocketServer } from "ws"
import { DropHandler } from "./environment"
import { assignPort } from "./ports"
import { callbackToPromise } from "./promise"

export class DummyServer implements DropHandler {
  servers: Array<Server | WebSocketServer> = []

  serveHelloWorld(): Promise<number> {
    const app = express()

    app.get('/', (req, res) => {
      res.send('Hello World!')
    })

    app.get('/host', (req, res) => {
      res.send(req.hostname)
    })

    const port = assignPort()

    return new Promise((accept, reject) => {
      this.servers.push(app.listen(port, () => {
        accept(port)
      }))
    })
  }

  serveWebSocket(): number {
    const port = assignPort()
    const server = new WebSocketServer({port})

    server.on('connection', (ws, req) => {
        ws.on('message', (data) => {
            ws.send(`echo: ${data}`)
        })
    })

    this.servers.push(server)

    return port
  }

  async drop() {
    for (const server of this.servers) {
      server.close()
    }
  }
}
