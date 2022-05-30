import * as express from "express"
import { Server } from "http"

export function runDevServer(port: number): Server {
    const app = express()
    
    app.get('/', (req, res) => {
      res.send('Hello World!')
    })
    
    return app.listen(port, () => {
      console.log(`Example app listening on port ${port}`)
    })
}
