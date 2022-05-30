var nextPort = 10200;

export function assignPort(): number {
    const port = nextPort++
    return port
}
