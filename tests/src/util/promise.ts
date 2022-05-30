type AcceptFunc = (accept: (error?: Error) => void) => void

export function callbackToPromise(acceptFunc: AcceptFunc): Promise<void> {
    return new Promise((accept, reject) => {
        acceptFunc((error) => {
            if (error === undefined) {
                accept
            } else {
                reject(error)
            }
        })
    })
}
