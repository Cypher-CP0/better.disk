// Mirrors bd_core::bytes::format_decimal so the map, list and details agree
// with the Rust side (decimal units; one decimal below 10; half-up; moves to
// the next unit when rounding reaches 1000).

const UNITS: readonly [string, number][] = [
  ["PB", 1e15],
  ["TB", 1e12],
  ["GB", 1e9],
  ["MB", 1e6],
  ["KB", 1e3],
  ["B", 1],
];

export function formatBytes(bytes: number): string {
  if (bytes < 1000) return `${Math.max(0, Math.round(bytes))} B`;
  let i = UNITS.findIndex(([, u]) => bytes >= u);
  if (i < 0) i = UNITS.length - 1;
  for (;;) {
    const [name, unit] = UNITS[i]!;
    const tenths = Math.floor((bytes * 10 + unit / 2) / unit);
    if (tenths < 100) return `${Math.floor(tenths / 10)}.${tenths % 10} ${name}`;
    const whole = Math.floor((bytes + unit / 2) / unit);
    if (whole < 1000 || i === 0) return `${whole} ${name}`;
    i -= 1;
  }
}

export function formatCount(n: number): string {
  return n.toLocaleString("en-US");
}
