import { Database } from 'sqlite3'

export class DroneDatabase {
    db: Database

    constructor(path: string) {
        this.db = new Database(path)
    }

    async addProxy(subdomain: string, address: string) {
        await this.db.run(`
            insert into route
            (subdomain, dest_address)
            values
            (?, ?)
        `, subdomain, address)
    }
}