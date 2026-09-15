function pad(n: number) {
  return String(n).padStart(2, '0')
}

/** Format RFC3339 / ISO / unix-seconds timestamps as local `YYYY-MM-DD HH:mm:ss`. */
export function formatTime(s?: string | number | null, fallback = '-') {
  if (s === undefined || s === null || s === '') return fallback
  const d = typeof s === 'number' ? new Date(s < 1e12 ? s * 1000 : s) : new Date(s)
  if (Number.isNaN(d.getTime())) return String(s)
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

export function formatBytes(n?: number | null) {
  if (!n) return '0 B'
  const u = ['B', 'KB', 'MB', 'GB', 'TB']
  let i = 0
  let v = n
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(v >= 10 || i === 0 ? 0 : 1)} ${u[i]}`
}
