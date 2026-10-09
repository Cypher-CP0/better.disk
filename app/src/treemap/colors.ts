// The green age scale (design document, "Shade and measurement").
// Shade encodes time only. Selection uses an independent outline; warnings
// use icons and text, never these colors.

export type BucketId =
  | "within7Days"
  | "days8To30"
  | "days31To90"
  | "days91To365"
  | "overOneYear"
  | "unknown";

export interface Bucket {
  id: BucketId;
  label: string;
  shade: string;
  fill: string;
  /** Unknown is gray with a pattern so it never reads as "old". */
  pattern?: string;
}

export const BUCKETS: readonly Bucket[] = [
  { id: "within7Days", label: "Within 7 days", shade: "Darkest", fill: "#14532d" },
  { id: "days8To30", label: "8 to 30 days", shade: "Dark", fill: "#1e6b3a" },
  { id: "days31To90", label: "31 to 90 days", shade: "Medium", fill: "#3d9a57" },
  { id: "days91To365", label: "91 to 365 days", shade: "Light", fill: "#8fd19a" },
  { id: "overOneYear", label: "Over 1 year", shade: "Lightest", fill: "#d3f0d3" },
  { id: "unknown", label: "Unknown", shade: "Gray with pattern", fill: "#c9cdca", pattern: "#a3a9a5" },
];

export function bucket(id: BucketId): Bucket {
  return BUCKETS.find((b) => b.id === id) ?? BUCKETS[5]!;
}

function channel(c: number): number {
  const s = c / 255;
  return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
}

/** WCAG 2.x relative luminance of `#rrggbb`. */
export function luminance(hex: string): number {
  const v = Number.parseInt(hex.slice(1), 16);
  const r = (v >> 16) & 255;
  const g = (v >> 8) & 255;
  const b = v & 255;
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

export function contrast(a: string, b: string): number {
  const la = luminance(a);
  const lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

export const LIGHT_TEXT = "#ffffff";
export const DARK_TEXT = "#111111";

/** White or dark text, whichever contrasts more with the fill. */
export function textColorFor(fill: string): string {
  return contrast(fill, LIGHT_TEXT) >= contrast(fill, DARK_TEXT) ? LIGHT_TEXT : DARK_TEXT;
}

const ORDER: readonly BucketId[] = [
  "within7Days",
  "days8To30",
  "days31To90",
  "days91To365",
  "overOneYear",
];

/**
 * Dominant date bucket by bytes (design document): the bucket holding the
 * most bytes; ties between known buckets choose the more recent; Unknown
 * wins when it is the largest or ties for largest.
 */
export function dominantBucket(entries: Iterable<{ bucket: BucketId; bytes: number }>): BucketId {
  const totals = new Map<BucketId, number>();
  for (const e of entries) totals.set(e.bucket, (totals.get(e.bucket) ?? 0) + e.bytes);
  let best: BucketId | null = null;
  let bestBytes = 0;
  for (const id of ORDER) {
    const b = totals.get(id) ?? 0;
    if (b > bestBytes) {
      best = id;
      bestBytes = b;
    }
  }
  const unknown = totals.get("unknown") ?? 0;
  if (best === null || unknown >= bestBytes) return "unknown";
  return best;
}
