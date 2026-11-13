export function bytes(n?: number | null): string {
  if (n == null) return "-";
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2)} ${units[i]}`;
}

export function when(iso?: string | null): string {
  if (!iso) return "-";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "-";
  const sameYear = d.getFullYear() === new Date().getFullYear();
  return d.toLocaleString(undefined, { month: "short", day: "numeric", year: sameYear ? undefined : "numeric", hour: "numeric", minute: "2-digit" });
}

export function duration(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  const m = Math.floor(s / 60);
  const h = Math.floor(m / 60);
  const pad = (v: number) => String(v).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m % 60)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
}

export function compositionLabel(c: { id: string; width?: number | null; height?: number | null; fps?: number | null; durationInFrames?: number | null }): string {
  const parts: string[] = [];
  if (c.width && c.height) parts.push(`${c.width}x${c.height}`);
  if (c.fps) parts.push(`${c.fps} fps`);
  if (c.durationInFrames) {
    parts.push(`${c.durationInFrames} frames`);
    if (c.fps) parts.push(`${(c.durationInFrames / c.fps).toFixed(2)} s`);
  }
  return parts.length ? `${c.id} (${parts.join(", ")})` : c.id;
}
