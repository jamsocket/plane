import { Database, open } from 'sqlite'
import * as sqlite3 from 'sqlite3'

export class DroneDatabase {
    private constructor(private db: Database) {}

    static async create(path: string): Promise<DroneDatabase> {
        const db = await open({
            filename: path,
            driver: sqlite3.Database,
        })

        return new DroneDatabase(db)
    }

    async addProxy(subdomain: string, address: string): Promise<void> {
        await this.db.run(`
                insert into route
                (subdomain, dest_address)
                values
                (?, ?)
                `, subdomain, address)
    }
}