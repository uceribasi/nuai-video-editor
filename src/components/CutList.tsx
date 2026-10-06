import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight, Info, Sparkles, Trash2, TriangleAlert } from "lucide-react";
import i18n from "../i18n";
import { useStore } from "../store";
import { KIND_COLOR, KIND_ORDER } from "../lib/cuts";
import { formatSeconds, formatTime } from "../lib/format";
import { player } from "../lib/player";
import type { Cut, CutKind } from "../lib/types";
import { Checkbox, IconButton, cx } from "./ui";

/** Localized explanation for a cut: rule cuts carry a message key, AI cuts free text. */
export function useCutReason() {
  const { t } = useTranslation();
  return useCallback(
    (c: Cut) =>
      c.code && i18n.exists(`reason.${c.code}`) ? t(`reason.${c.code}`, { detail: c.detail }) : c.reason || t(`kinds.${c.kind}`),
    [t],
  );
}

const PAGE = 60;

function CutRow({ cut, selected }: { cut: Cut; selected: boolean }) {
  const { t } = useTranslation();
  const reasonOf = useCutReason();
  const toggleCut = useStore((s) => s.toggleCut);
  const selectCut = useStore((s) => s.selectCut);
  const removeCut = useStore((s) => s.removeCut);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (selected) ref.current?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  return (
    <div
      ref={ref}
      onClick={() => {
        selectCut(cut.id);
        player.seek(Math.max(0, cut.start - 0.4));
      }}
      className={cx(
        "group flex cursor-default gap-2.5 rounded-md px-2 py-1.5",
        selected ? "bg-accent-soft" : "hover:bg-hover",
        !cut.enabled && "opacity-55",
      )}
    >
      <div className="pt-0.5">
        <Checkbox checked={cut.enabled} onChange={(v) => toggleCut(cut.id, v)} color={KIND_COLOR[cut.kind]} />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 font-mono text-[11px] text-muted">
          <span>{formatTime(cut.start, true)}</span>
          <span className="text-faint">{formatSeconds(cut.end - cut.start)}</span>
          {cut.source === "ai" && (
            <span className="inline-flex items-center gap-0.5 rounded bg-accent-soft px-1 font-sans text-[10px] font-medium text-accent">
              <Sparkles size={9} />
              {t("cuts.source.ai")}
            </span>
          )}
          {cut.source !== "user" && cut.confidence < 0.8 && (
            <span className="font-sans text-[10px] text-faint">{t("cuts.sure", { pct: Math.round(cut.confidence * 100) })}</span>
          )}
        </div>
        <div className="line-clamp-2 text-[12.5px] leading-snug text-fg/90">{reasonOf(cut)}</div>
      </div>
      {cut.source === "user" && (
        <IconButton
          label={t("common.remove")}
          className="h-6 w-6 opacity-0 group-hover:opacity-100"
          onClick={(e) => {
            e.stopPropagation();
            removeCut(cut.id);
          }}
        >
          <Trash2 size={13} />
        </IconButton>
      )}
    </div>
  );
}

function Group({ kind, cuts, selectedId }: { kind: CutKind; cuts: Cut[]; selectedId: string | null }) {
  const { t } = useTranslation();
  const setKindEnabled = useStore((s) => s.setKindEnabled);
  const [open, setOpen] = useState(kind !== "silence");
  const [limit, setLimit] = useState(PAGE);
  const enabled = cuts.filter((c) => c.enabled);
  const removed = enabled.reduce((s, c) => s + (c.end - c.start), 0);
  const hasSelected = cuts.some((c) => c.id === selectedId);

  useEffect(() => {
    if (hasSelected) {
      setOpen(true);
      const idx = cuts.findIndex((c) => c.id === selectedId);
      if (idx >= limit) setLimit(idx + PAGE);
    }
  }, [hasSelected, selectedId, cuts, limit]);

  return (
    <div className="border-b border-line/60 py-1">
      <div className="flex items-center gap-2 px-3 py-1.5">
        <Checkbox
          checked={enabled.length === cuts.length}
          indeterminate={enabled.length > 0 && enabled.length < cuts.length}
          onChange={(v) => setKindEnabled(kind, v)}
          color={KIND_COLOR[kind]}
          label={t("cuts.toggleGroup")}
        />
        <button type="button" className="flex flex-1 items-center gap-1.5 text-left" onClick={() => setOpen(!open)}>
          <span className="font-medium">{t(`kinds.${kind}`)}</span>
          <span className="text-xs text-faint">
            {enabled.length}/{cuts.length}
          </span>
          <span className="ml-auto font-mono text-xs text-muted">−{formatSeconds(removed)}</span>
          {open ? <ChevronDown size={14} className="text-faint" /> : <ChevronRight size={14} className="text-faint" />}
        </button>
      </div>
      {open && (
        <div className="flex flex-col px-1.5 pb-1">
          {cuts.slice(0, limit).map((c) => (
            <CutRow key={c.id} cut={c} selected={c.id === selectedId} />
          ))}
          {cuts.length > limit && (
            <button type="button" className="px-2 py-1.5 text-left text-xs text-accent" onClick={() => setLimit(limit + PAGE)}>
              {t("cuts.showMore", { count: cuts.length - limit })}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

export function CutList() {
  const { t } = useTranslation();
  const project = useStore((s) => s.project);
  const selectedId = useStore((s) => s.selectedCutId);

  const groups = useMemo(() => {
    const by = new Map<CutKind, Cut[]>();
    for (const c of project?.cuts ?? []) {
      if (!by.has(c.kind)) by.set(c.kind, []);
      by.get(c.kind)!.push(c);
    }
    return KIND_ORDER.filter((k) => by.has(k)).map((k) => ({ kind: k, cuts: by.get(k)! }));
  }, [project]);

  return (
    <div className="min-h-0 flex-1 overflow-y-auto pt-2">
        {project?.warnings.map((w, i) => (
          <div key={i} className="mx-3 mb-2 flex gap-2 rounded-md bg-warn/10 p-2.5 text-xs text-warn">
            <TriangleAlert size={14} className="mt-0.5 shrink-0" />
            <span className="selectable">{t(`warning.${w.code}`, { message: w.message, defaultValue: w.message })}</span>
          </div>
        ))}
        {project && project.notes.length > 0 && (
          <div className="mx-3 mb-2 rounded-md border border-accent/30 bg-accent-soft p-2.5 text-xs">
            <div className="mb-1 flex items-center gap-1.5 font-medium text-accent">
              <Info size={13} />
              {t("cuts.notes")}
            </div>
            {project.notes.map((n, i) => (
              <p key={i} className="selectable text-fg/90">
                {n}
              </p>
            ))}
            {project.aiModel && <p className="mt-1 text-faint">{t("cuts.model", { model: project.aiModel })}</p>}
          </div>
        )}
        {!project && <p className="px-4 py-6 text-center text-muted">{t("cuts.empty")}</p>}
        {groups.map((g) => (
          <Group key={g.kind} kind={g.kind} cuts={g.cuts} selectedId={selectedId} />
        ))}
    </div>
  );
}
