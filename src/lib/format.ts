/** 75.4 → "1:15", 3725 → "1:02:05" */
export function formatTime(seconds: number, withTenths = false): string {
  const s = Math.max(0, seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = Math.floor(s % 60);
  const pad = (n: number) => n.toString().padStart(2, "0");
  let out = h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${m}:${pad(sec)}`;
  if (withTenths) out += `.${Math.floor((s % 1) * 10)}`;
  return out;
}

/** "1:30" / "90" / "1:02:05" → seconds; NaN when unparsable. */
export function parseTime(text: string): number {
  const parts = text.trim().split(":").map(Number);
  if (!parts.length || parts.some((p) => Number.isNaN(p) || p < 0)) return NaN;
  return parts.reduce((acc, p) => acc * 60 + p, 0);
}

export function formatSeconds(seconds: number): string {
  return seconds < 60 ? `${seconds.toFixed(1)}s` : formatTime(seconds);
}

export function formatBytes(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(0)} MB`;
  return `${(bytes / 1e3).toFixed(0)} KB`;
}

export function basename(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
