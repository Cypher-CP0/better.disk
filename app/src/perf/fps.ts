// Frame statistics for the 1,500-rectangle interaction target (60 fps on
// the baseline machine, with a responsive fallback).

export interface FrameStats {
  frames: number;
  durationMs: number;
  fps: number;
  p50FrameMs: number;
  p95FrameMs: number;
  /** Frames longer than 1.5x a 60 Hz frame (25 ms). */
  longFrames: number;
  /** Time spent in our own hit test + draw per frame. */
  drawP50Ms: number;
  drawP95Ms: number;
  blocks: number;
}

function percentile(sorted: number[], p: number): number {
  if (sorted.length === 0) return 0;
  const idx = Math.min(sorted.length - 1, Math.max(0, Math.round((sorted.length - 1) * p)));
  return sorted[idx]!;
}

export function summarizeFrames(
  frameDeltas: readonly number[],
  drawTimes: readonly number[],
  blocks: number,
): FrameStats {
  const deltas = [...frameDeltas].sort((a, b) => a - b);
  const draws = [...drawTimes].sort((a, b) => a - b);
  const duration = frameDeltas.reduce((a, b) => a + b, 0);
  return {
    frames: frameDeltas.length,
    durationMs: duration,
    fps: duration > 0 ? (frameDeltas.length * 1000) / duration : 0,
    p50FrameMs: percentile(deltas, 0.5),
    p95FrameMs: percentile(deltas, 0.95),
    longFrames: frameDeltas.filter((d) => d > 25).length,
    drawP50Ms: percentile(draws, 0.5),
    drawP95Ms: percentile(draws, 0.95),
    blocks,
  };
}
