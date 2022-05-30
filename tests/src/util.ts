var nextPort = 10200;
export function assignPort(): number {
    const port = nextPort++
    return port
}

export function sleep(durationMillis: number): Promise<void> {
    return new Promise((resolve) => setInterval(resolve, durationMillis))
}
