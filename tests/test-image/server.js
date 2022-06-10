const express = require('express');
const { exit } = require('process');
const port = process.env.PORT || 8080
const app = express()

const argv = require('minimist')(process.argv.slice(2));

if (argv['exit'] !== undefined) {
  // If --exit is passed, crash immediately with an exit code.
  exit(argv['exit'])
}

app.get('/', (req, res) => {
  res.send('Hello World!')
})

app.get('/exit/:code', (req, res) => {
  exit(parseInt(req.params.code))
})

app.listen(port, () => {
  console.log(`Example app listening on port ${port}`)
})
