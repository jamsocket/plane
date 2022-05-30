import { Database } from 'sqlite3'
import { sleep } from './sleep'

export class DroneDatabase {
    db: Database

    constructor(path: string) {
        this.db = new Database(path)
    }

    addProxy(subdomain: string, address: string): Promise<void> {
        return new Promise((accept, reject) => {
            let callback = (error?: Error) => {
                if (error === null) {
                    accept()
                } else {
                    reject(error)
                }
            }

            this.db.run(`
                insert into route
                (subdomain, dest_address)
                values
                (?, ?)
            `, subdomain, address, callback)
        })
    }
}