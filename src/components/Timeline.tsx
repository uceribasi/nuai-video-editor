import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ZoomIn, ZoomOut } from "lucide-react";
import { useStore } from "../store";
import { KIND_COLOR, mergedCuts, rangeAt } from "../lib/cuts";
import { formatTime } from "../lib/format";
import { player } from "../lib/player";
import type { Cut } from "../lib/types";
import { Button, IconButton } from "./ui";
import { useCutReason } from "./CutList";

const HEIGHT = 128;
const RULER = 20;
const MAX_ZOOM = 600; // px per second
const TICKS = [0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600];

type Drag =
  | { kind: "scrub" }
  | { kind: "create"; from: number; to: number };

export function Timeline() {
  const { t } = useTranslation();
  const reasonOf = useCutReason();
  const media = useStore((s) => s.media);
  const waveform = useStore((s) => s.waveform);
  const rate = useStore((s) => s.waveformRate);
  const project = useStore((s) => s.project);
  const selectedId = useStore((s) => s.selectedCutId);
  const selectCut = useStore((s) => s.selectCut);
  const addManualCut = useStore((s) => s.addManualCut);

  const scrollRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [width, setWidth] = useState(800);
  const [zoom, setZoom] = useState(0);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [hover, setHover] = useState<{ x: number; cut: Cut } | null>(null);
  const dirty = useRef(true);

  const duration = media?.duration ?? 0;
  const cuts = project?.cuts ?? [];
  const merged = useMemo(() => mergedCuts(cuts, duration), [cuts, duration]);
  const fitZoom = duration > 0 ? width / duration : 1;
  const z = Math.max(zoom || fitZoom, fitZoom);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  useEffect(() => setZoom(0), [media?.path]);
  useEffect(() => {
    dirty.current = true;
  }, [z, width, cuts, selectedId, drag, waveform]);

  const timeAt = useCallback(
    (clientX: number) => {
      const el = scrollRef.current!;
      const x = clientX - el.getBoundingClientRect().left + el.scrollLeft;
      return Math.min(duration, Math.max(0, x / z));
    },
    [z, duration],
  );

  const cutAt = useCallback(
    (time: number): Cut | undefined => {
      // Prefer the shortest cut under the cursor so small cuts inside pauses stay clickable.
      let best: Cut | undefined;
      for (const c of cuts) {
        if (time >= c.start && time <= c.end && (!best || c.end - c.start < best.end - best.start)) best = c;
      }
      return best;
    },
    [cuts],
  );

  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    const el = scrollRef.current;
    if (!canvas || !el || duration <= 0) return;
    const dpr = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(width * dpr) || canvas.height !== HEIGHT * dpr) {
      canvas.width = Math.round(width * dpr);
      canvas.height = HEIGHT * dpr;
    }
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, HEIGHT);
    const scroll = el.scrollLeft;
    const t0 = scroll / z;
    const t1 = (scroll + width) / z;
    const xOf = (time: number) => time * z - scroll;

    // Ruler
    ctx.fillStyle = "#121419";
    ctx.fillRect(0, 0, width, RULER);
    const step = TICKS.find((s) => s * z >= 70) ?? 3600;
    ctx.fillStyle = "#6b7280";
    ctx.strokeStyle = "#2a2e38";
    ctx.font = "10px ui-monospace, SF Mono, Menlo, monospace";
    ctx.textBaseline = "middle";
    for (let s = Math.floor(t0 / step) * step; s <= t1; s += step) {
      const x = Math.round(xOf(s)) + 0.5;
      ctx.beginPath();
      ctx.moveTo(x, RULER - 6);
      ctx.lineTo(x, RULER);
      ctx.stroke();
      ctx.fillText(step < 1 ? formatTime(s, true) : formatTime(s), x + 4, RULER / 2);
    }
    ctx.beginPath();
    ctx.moveTo(0, RULER + 0.5);
    ctx.lineTo(width, RULER + 0.5);
    ctx.stroke();

    // Waveform, dimmed where it will be cut
    const mid = RULER + (HEIGHT - RULER) / 2;
    const amp = (HEIGHT - RULER) / 2 - 6;
    for (let x = 0; x < width; x++) {
      const a = (scroll + x) / z;
      const b = (scroll + x + 1) / z;
      const i0 = Math.floor(a * rate);
      const i1 = Math.max(i0 + 1, Math.ceil(b * rate));
      let peak = 0;
      for (let i = i0; i < i1 && i < waveform.length; i++) peak = Math.max(peak, waveform[i]);
      const h = Math.max(1, peak * amp);
      ctx.fillStyle = rangeAt(merged, (a + b) / 2) ? "#353a46" : "#8d95a8";
      ctx.fillRect(x, mid - h, 1, h * 2);
    }

    // Cuts
    for (const c of cuts) {
      if (c.end < t0 || c.start > t1) continue;
      const x0 = xOf(c.start);
      const w = Math.max(2, (c.end - c.start) * z);
      const color = KIND_COLOR[c.kind];
      if (c.enabled) {
        ctx.fillStyle = color + "2e";
        ctx.fillRect(x0, RULER + 1, w, HEIGHT - RULER - 1);
        ctx.fillStyle = color;
        ctx.fillRect(x0, RULER + 1, w, 4);
      } else {
        ctx.setLineDash([3, 3]);
        ctx.strokeStyle = color + "aa";
        ctx.strokeRect(x0 + 0.5, RULER + 1.5, w - 1, HEIGHT - RULER - 3);
        ctx.setLineDash([]);
      }
      if (c.id === selectedId) {
        ctx.strokeStyle = "#ffffff";
        ctx.lineWidth = 1.5;
        ctx.strokeRect(x0 + 0.75, RULER + 1.75, w - 1.5, HEIGHT - RULER - 3.5);
        ctx.lineWidth = 1;
      }
    }

    // Range being drawn with shift-drag
    if (drag?.kind === "create") {
      const a = Math.min(drag.from, drag.to);
      const b = Math.max(drag.from, drag.to);
      ctx.fillStyle = KIND_COLOR.manual + "40";
      ctx.fillRect(xOf(a), RULER, (b - a) * z, HEIGHT - RULER);
    }

    // Playhead
    const now = player.el?.currentTime ?? 0;
    const px = Math.round(xOf(now)) + 0.5;
    ctx.strokeStyle = "#ffffff";
    ctx.beginPath();
    ctx.moveTo(px, 0);
    ctx.lineTo(px, HEIGHT);
    ctx.stroke();
    ctx.fillStyle = "#ffffff";
    ctx.beginPath();
    ctx.moveTo(px - 5, 0);
    ctx.lineTo(px + 5, 0);
    ctx.lineTo(px, 7);
    ctx.fill();
  }, [width, z, duration, merged, cuts, rate, waveform, selectedId, drag]);

  // Redraw when something changed or the playhead moved; follow the playhead while playing.
  useEffect(() => {
    let raf = 0;
    let lastTime = -1;
    let lastScroll = -1;
    const loop = () => {
      const el = scrollRef.current;
      const v = player.el;
      const now = v?.currentTime ?? 0;
      if (el && v && !v.paused) {
        const x = now * z - el.scrollLeft;
        if (x > width - 40 || x < 0) el.scrollLeft = Math.max(0, now * z - width * 0.15);
      }
      if (dirty.current || now !== lastTime || (el && el.scrollLeft !== lastScroll)) {
        dirty.current = false;
        lastTime = now;
        lastScroll = el?.scrollLeft ?? 0;
        draw();
      }
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [draw, z, width]);

  // Zoom with ctrl/cmd + wheel around the cursor; plain vertical wheel scrolls sideways.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (e.ctrlKey || e.metaKey) {
        e.preventDefault();
        const rect = el.getBoundingClientRect();
        const mx = e.clientX - rect.left;
        const at = (el.scrollLeft + mx) / z;
        const next = Math.min(MAX_ZOOM, Math.max(fitZoom, z * Math.exp(-e.deltaY * 0.004)));
        setZoom(next);
        requestAnimationFrame(() => {
          el.scrollLeft = at * next - mx;
        });
      } else if (Math.abs(e.deltaY) > Math.abs(e.deltaX)) {
        e.preventDefault();
        el.scrollLeft += e.deltaY;
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [z, fitZoom]);

  const zoomBy = (factor: number) => {
    const el = scrollRef.current;
    if (!el) return;
    const center = (el.scrollLeft + width / 2) / z;
    const next = Math.min(MAX_ZOOM, Math.max(fitZoom, z * factor));
    setZoom(next);
    requestAnimationFrame(() => {
      el.scrollLeft = center * next - width / 2;
    });
  };

  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    const time = timeAt(e.clientX);
    if (e.shiftKey) {
      setDrag({ kind: "create", from: time, to: time });
      return;
    }
    const inRuler = e.clientY - (scrollRef.current?.getBoundingClientRect().top ?? 0) < RULER;
    const cut = inRuler ? undefined : cutAt(time);
    if (cut) {
      selectCut(cut.id);
      player.seek(Math.max(0, cut.start - 0.4));
    } else {
      selectCut(null);
      player.seek(time);
      setDrag({ kind: "scrub" });
    }
  };

  useEffect(() => {
    if (!drag) return;
    const onMove = (e: MouseEvent) => {
      const time = timeAt(e.clientX);
      if (drag.kind === "scrub") player.seek(time);
      else setDrag({ ...drag, to: time });
    };
    const onUp = () => {
      if (drag.kind === "create") {
        const a = Math.min(drag.from, drag.to);
        const b = Math.max(drag.from, drag.to);
        if (b - a > 0.05) addManualCut(a, b);
      }
      setDrag(null);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [drag, timeAt, addManualCut]);

  const onMouseMove = (e: React.MouseEvent) => {
    if (drag) return;
    const cut = cutAt(timeAt(e.clientX));
    const rect = scrollRef.current!.getBoundingClientRect();
    setHover(cut ? { x: e.clientX - rect.left, cut } : null);
  };

  if (!media) return null;

  return (
    <div className="shrink-0 border-t border-line bg-panel">
      <div className="flex h-8 items-center gap-2 px-3 text-xs text-faint">
        <span className="truncate">{t("timeline.hint")}</span>
        <div className="ml-auto flex items-center gap-0.5">
          <IconButton label="Zoom out" onClick={() => zoomBy(1 / 1.6)} className="h-6 w-6">
            <ZoomOut size={14} />
          </IconButton>
          <Button size="sm" variant="ghost" onClick={() => setZoom(0)}>
            {t("timeline.fit")}
          </Button>
          <IconButton label="Zoom in" onClick={() => zoomBy(1.6)} className="h-6 w-6">
            <ZoomIn size={14} />
          </IconButton>
        </div>
      </div>
      <div className="relative">
        <div
          ref={scrollRef}
          className="overflow-x-auto overflow-y-hidden"
          style={{ height: HEIGHT + 12 }}
          onMouseDown={onMouseDown}
          onMouseMove={onMouseMove}
          onMouseLeave={() => setHover(null)}
        >
          <div style={{ width: Math.max(width, duration * z), height: HEIGHT }}>
            <canvas ref={canvasRef} className="sticky left-0 block" style={{ width, height: HEIGHT }} />
          </div>
        </div>
        {hover && (
          <div
            className="pointer-events-none absolute top-6 z-10 max-w-[280px] -translate-x-1/2 rounded-md border border-line bg-raised px-2 py-1 text-xs shadow-lg"
            style={{ left: Math.min(Math.max(hover.x, 140), width - 140) }}
          >
            <span className="mr-1.5 inline-block h-2 w-2 rounded-full" style={{ background: KIND_COLOR[hover.cut.kind] }} />
            <span className="font-medium">{t(`kinds.${hover.cut.kind}`)}</span>
            <span className="text-muted"> · {reasonOf(hover.cut)}</span>
          </div>
        )}
      </div>
    </div>
  );
}
