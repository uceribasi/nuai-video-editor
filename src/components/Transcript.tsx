import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Scissors, Undo2 } from "lucide-react";
import { useStore } from "../store";
import { KIND_COLOR, cutsAtPoints } from "../lib/cuts";
import { formatTime } from "../lib/format";
import { player } from "../lib/player";
import type { Word } from "../lib/types";
import { Button } from "./ui";

interface FlatWord extends Word {
  index: number;
}

export function Transcript() {
  const { t } = useTranslation();
  const project = useStore((s) => s.project);
  const addManualCut = useStore((s) => s.addManualCut);
  const keepRange = useStore((s) => s.keepRange);
  const selectCut = useStore((s) => s.selectCut);
  const containerRef = useRef<HTMLDivElement>(null);
  const [selection, setSelection] = useState<{ from: number; to: number; x: number; y: number } | null>(null);

  const transcript = project?.transcript;
  const { paragraphs, words } = useMemo(() => {
    const words: FlatWord[] = [];
    const paragraphs: { id: string; start: number; words: FlatWord[] }[] = [];
    for (const u of transcript?.utterances ?? []) {
      const ws = u.words.map((w, i) => ({ ...w, index: words.length + i }));
      words.push(...ws);
      paragraphs.push({ id: u.id, start: u.start, words: ws });
    }
    return { paragraphs, words };
  }, [transcript]);

  const wordCuts = useMemo(
    () => cutsAtPoints(words.map((w) => (w.start + w.end) / 2), project?.cuts ?? []),
    [words, project?.cuts],
  );

  // Highlight the word under the playhead without re-rendering the whole transcript.
  useEffect(() => {
    let raf = 0;
    let current = -1;
    const loop = () => {
      const now = player.el?.currentTime ?? 0;
      let lo = 0;
      let hi = words.length - 1;
      let found = -1;
      while (lo <= hi) {
        const mid = (lo + hi) >> 1;
        if (words[mid].start <= now) {
          found = mid;
          lo = mid + 1;
        } else hi = mid - 1;
      }
      if (found !== -1 && now > words[found].end + 0.3) found = -1;
      if (found !== current) {
        const root = containerRef.current;
        root?.querySelector(".word.is-current")?.classList.remove("is-current");
        if (found !== -1 && root) {
          const el = root.querySelector(`[data-w="${found}"]`);
          el?.classList.add("is-current");
          if (el && player.el && !player.el.paused) {
            const r = el.getBoundingClientRect();
            const c = root.getBoundingClientRect();
            if (r.top < c.top || r.bottom > c.bottom) el.scrollIntoView({ block: "center", behavior: "smooth" });
          }
        }
        current = found;
      }
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [words]);

  const onMouseUp = () => {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || !containerRef.current) {
      setSelection(null);
      return;
    }
    const indexOf = (node: Node | null) => {
      const el = node instanceof Element ? node : node?.parentElement;
      const w = el?.closest("[data-w]");
      return w ? Number(w.getAttribute("data-w")) : NaN;
    };
    const a = indexOf(sel.anchorNode);
    const b = indexOf(sel.focusNode);
    if (Number.isNaN(a) || Number.isNaN(b)) return setSelection(null);
    const rect = sel.getRangeAt(0).getBoundingClientRect();
    const box = containerRef.current.getBoundingClientRect();
    setSelection({
      from: Math.min(a, b),
      to: Math.max(a, b),
      x: rect.left + rect.width / 2 - box.left,
      y: rect.top - box.top + containerRef.current.scrollTop,
    });
  };

  const applySelection = (cut: boolean) => {
    if (!selection) return;
    const start = words[selection.from].start;
    const end = words[selection.to].end;
    if (cut) addManualCut(start, end);
    else keepRange(start, end);
    window.getSelection()?.removeAllRanges();
    setSelection(null);
  };

  if (!transcript) {
    return <div className="flex flex-1 items-center justify-center p-6 text-center text-muted">{t("transcript.empty")}</div>;
  }

  return (
    <div
      ref={containerRef}
      className="selectable relative min-h-0 flex-1 overflow-y-auto px-6 py-4 text-[14.5px] leading-[1.9]"
      onMouseUp={onMouseUp}
      onScroll={() => selection && setSelection(null)}
    >
      {paragraphs.map((p) => (
        <p key={p.id} className="mb-2 flex gap-3">
          <button
            type="button"
            className="mt-[5px] h-fit shrink-0 font-mono text-[10.5px] leading-5 text-faint hover:text-accent"
            onClick={() => player.seek(p.start)}
          >
            {formatTime(p.start)}
          </button>
          <span>
            {p.words.map((w) => {
              const cut = wordCuts[w.index];
              const cls = cut ? (cut.enabled ? " is-cut" : " is-proposed") : "";
              return (
                <span key={w.index}>
                  <span
                    data-w={w.index}
                    className={`word${cls}`}
                    style={cut ? { textDecorationColor: KIND_COLOR[cut.kind] } : undefined}
                    onClick={() => {
                      if (!window.getSelection()?.isCollapsed) return;
                      player.seek(w.start);
                      if (cut) selectCut(cut.id);
                    }}
                  >
                    {w.text}
                  </span>{" "}
                </span>
              );
            })}
          </span>
        </p>
      ))}
      {selection && (
        <div
          className="absolute z-10 flex -translate-x-1/2 -translate-y-full gap-1 rounded-lg border border-line bg-raised p-1 shadow-xl"
          style={{ left: selection.x, top: selection.y - 6 }}
          onMouseUp={(e) => e.stopPropagation()}
        >
          <Button size="sm" variant="ghost" onClick={() => applySelection(true)}>
            <Scissors size={13} />
            {t("transcript.cut")}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => applySelection(false)}>
            <Undo2 size={13} />
            {t("transcript.keep")}
          </Button>
        </div>
      )}
    </div>
  );
}
