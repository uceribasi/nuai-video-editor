import { useTranslation } from "react-i18next";
import { useStore, type RightTab } from "../store";
import { CutList } from "./CutList";
import { SubtitlePanel } from "./SubtitlePanel";
import { cx } from "./ui";

export function RightPanel() {
  const { t } = useTranslation();
  const tab = useStore((s) => s.rightTab);
  const setTab = useStore((s) => s.setRightTab);
  const project = useStore((s) => s.project);
  const enabled = project?.cuts.filter((c) => c.enabled).length ?? 0;
  const tracks = project?.subtitles?.tracks.length ?? 0;

  const tabs: { id: RightTab; label: string; badge?: string }[] = [
    { id: "cuts", label: t("cuts.title"), badge: project ? `${enabled}/${project.cuts.length}` : undefined },
    { id: "subtitles", label: t("subtitles.title"), badge: tracks ? String(tracks) : undefined },
  ];

  return (
    <aside className="flex w-[340px] shrink-0 flex-col border-l border-line bg-panel">
      <div className="flex gap-1 border-b border-line px-2 pt-2">
        {tabs.map((x) => (
          <button
            key={x.id}
            type="button"
            onClick={() => setTab(x.id)}
            className={cx(
              "-mb-px flex items-center gap-1.5 border-b-2 px-3 pb-2 pt-1 text-[13px] font-medium",
              tab === x.id ? "border-accent text-fg" : "border-transparent text-muted hover:text-fg",
            )}
          >
            {x.label}
            {x.badge && <span className="rounded bg-raised px-1.5 font-mono text-[10.5px] text-muted">{x.badge}</span>}
          </button>
        ))}
      </div>
      {tab === "cuts" ? <CutList /> : <SubtitlePanel />}
    </aside>
  );
}
