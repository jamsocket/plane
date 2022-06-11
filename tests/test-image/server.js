const express = require("express")
const { exit } = require("process")
const port = process.env.PORT || 8080
const exitCode = process.env.EXIT_CODE || null
const exitTimeout = process.env.EXIT_TIMEOUT || 0
const app = express()

if (exitCode !== null) {
  // If EXIT_CODE is present, exit immediately with
  // its value as a code.
  console.log(`Crashing intentionally because EXIT_CODE was set to ${exitCode}.`)
  setTimeout(() => exit(parseInt(exitCode, 10)), exitTimeout)
} else {
  app.get("/", (req, res) => {
    res.send("Hello World!")
  })

  app.get('/exit/:code', (req) => {
    exit(parseInt(req.params.code, 10))
  })

  app.listen(port, () => {
    console.log(`Example app listening on port ${port}`)
  })
}
