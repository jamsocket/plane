export function generateId(prefix?: string): string {
  return (prefix ? (prefix + '-') : '') + Math.random().toString(36).slice(2, 8)
}
