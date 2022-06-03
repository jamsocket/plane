export function sleep(durationMillis: number): Promise<void> {
  return new Promise((resolve) => setInterval(resolve, durationMillis));
}
