/**
 * GitHub-style relative time: "just now", "5 minutes ago", "2 months ago".
 * Falls back to the absolute date for anything older than a year.
 */
export function relativeTime(iso: string, now: Date = new Date()): string {
  const then = new Date(iso)
  const secs = Math.floor((now.getTime() - then.getTime()) / 1000)
  if (Number.isNaN(secs)) return iso
  if (secs < 60) return 'just now'
  const units: [number, string][] = [
    [60, 'minute'],
    [3600, 'hour'],
    [86400, 'day'],
    [86400 * 30, 'month'],
    [86400 * 365, 'year'],
  ]
  let value = secs
  let unit = 'minute'
  for (let i = units.length - 1; i >= 0; i--) {
    const [divisor, name] = units[i]
    if (secs >= divisor) {
      value = Math.floor(secs / divisor)
      unit = name
      break
    }
  }
  const plural = value === 1 ? unit : `${unit}s`
  return `${value} ${plural} ago`
}
