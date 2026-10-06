import type { Cut, CutKind } from "./types";

/** Same rule as the Rust side: fragments shorter than this are not worth keeping. */
const MIN_KEEP = 0.12;

export const KIND_ORDER: CutKind[] = [
  "retake",
  "mistake",
  "filler",
  "stutter",
  "static",
  "offtopic",
  "shorten",
  "manual",
  "silence",
];

export const KIND_COLOR: Record<CutKind, string> = {
  silence: "#64748b",
  retake: "#f59e0b",
  filler: "#a78bfa",
  stutter: "#f472b6",
  static: "#22d3ee",
  mistake: "#f87171",
  offtopic: "#fb923c",
  shorten: "#2dd4bf",
  manual: "#60a5fa",
};

export type Range = [number, number];

/** Union of enabled cuts, with tiny kept slivers absorbed. */
export function mergedCuts(cuts: Cut[], duration: number): Range[] {
  const ranges = cuts
    .filter((c) => c.enabled)
    .map((c) => [Math.max(0, c.start), Math.min(duration, c.end)] as Range)
    .filter(([s, e]) => e > s)
    .sort((a, b) => a[0] - b[0]);
  const merged: Range[] = [];
  for (const [s, e] of ranges) {
    const last = merged[merged.length - 1];
    if (last && s <= last[1] + MIN_KEEP) last[1] = Math.max(last[1], e);
    else merged.push([s, e]);
  }
  if (merged.length) {
    if (merged[0][0] < MIN_KEEP) merged[0][0] = 0;
    const last = merged[merged.length - 1];
    if (duration - last[1] < MIN_KEEP) last[1] = duration;
  }
  return merged;
}

/** Fingerprint of the effective edit, used to tell when subtitles are out of date. */
export function cutsSignature(cuts: Cut[], duration: number): string {
  return mergedCuts(cuts, duration)
    .map(([s, e]) => `${s.toFixed(2)}-${e.toFixed(2)}`)
    .join(",");
}

export function keepSegments(cuts: Cut[], duration: number): Range[] {
  const keeps: Range[] = [];
  let t = 0;
  for (const [s, e] of mergedCuts(cuts, duration)) {
    if (s > t) keeps.push([t, s]);
    t = e;
  }
  if (duration > t) keeps.push([t, duration]);
  return keeps.filter(([s, e]) => e - s >= MIN_KEEP);
}

export function editedDuration(cuts: Cut[], duration: number): number {
  return keepSegments(cuts, duration).reduce((sum, [s, e]) => sum + (e - s), 0);
}

/** The merged cut range containing `t`, if any (ranges must be sorted). */
export function rangeAt(ranges: Range[], t: number): Range | undefined {
  let lo = 0;
  let hi = ranges.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const [s, e] = ranges[mid];
    if (t < s) hi = mid - 1;
    else if (t >= e) lo = mid + 1;
    else return ranges[mid];
  }
  return undefined;
}

/**
 * For each sorted time point, the most relevant cut covering it: enabled beats disabled,
 * speech cuts beat silence (they explain why words disappear).
 */
export function cutsAtPoints(points: number[], cuts: Cut[]): (Cut | undefined)[] {
  const sorted = [...cuts].sort((a, b) => a.start - b.start);
  const result: (Cut | undefined)[] = [];
  let active: Cut[] = [];
  let next = 0;
  const score = (c: Cut) => (c.enabled ? 2 : 0) + (c.kind === "silence" ? 0 : 1);
  for (const p of points) {
    while (next < sorted.length && sorted[next].start <= p) active.push(sorted[next++]);
    active = active.filter((c) => c.end > p);
    let best: Cut | undefined;
    for (const c of active) if (!best || score(c) > score(best)) best = c;
    result.push(best);
  }
  return result;
}

let manualCounter = 0;
export function manualCut(start: number, end: number): Cut {
  manualCounter += 1;
  return {
    id: `manual-${Date.now().toString(36)}-${manualCounter}`,
    start,
    end,
    kind: "manual",
    reason: "",
    code: "manual",
    detail: "",
    confidence: 1,
    enabled: true,
    source: "user",
  };
}
