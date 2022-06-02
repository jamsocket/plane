import express from "express"
import { Server } from "http"
import { WebSocketServer } from "ws"
import { DropHandler } from "./environment"
import getPort from "@ava/get-port"

export class DummyServer implements DropHandler {
  servers: Array<Server | WebSocketServer> = []

  async serveHelloWorld(): Promise<number> {
    const app = express()

    app.get('/', (req, res) => {
      res.send('Hello World!')
    })

    app.get('/host', (req, res) => {
      res.send(req.hostname)
    })

    const port = await getPort()

    return await new Promise((accept, reject) => {
      this.servers.push(app.listen(port, () => {
        accept(port)
      }))
    })
  }

  async serveWebSocket(): Promise<number> {
    const port = await getPort()
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
