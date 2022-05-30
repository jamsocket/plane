#!/usr/bin/env zx

const tempDir = (await $`mktemp -d`).stdout.trim()
const db = `${tempDir}/data.db`
const connString = `sqlite://${db}`

await $`sqlite3 ${db} "VACUUM;"`

process.env.DATABASE_URL = connString

await $`sqlx migrate run --database-url ${connString}`

await $`cargo sqlx prepare --database-url ${connString}`
