#!/bin/sh

set -e

TEMPDIR=$(mktemp -d)
DB="${TEMPDIR}/data.db"
export DATABASE_URL="sqlite://${DB}"

sqlite3 ${DB} "VACUUM;"

sqlx migrate run --database-url ${DATABASE_URL}

cargo sqlx prepare --database-url ${DATABASE_URL}
