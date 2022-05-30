import { Database } from 'sqlite3'

export class DroneDatabase {
    db: Database

    constructor(path: string) {
        this.db = new Database(path)
    }

    addProxy(subdomain: string, address: string) {
        this.db.run(`
            insert into route
            (subdomain, dest_address)
            values
            (?, ?)
        `, subdomain, address)
    }
}