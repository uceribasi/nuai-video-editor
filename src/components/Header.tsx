import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { FolderOpen, Settings as SettingsIcon, Upload } from "lucide-react";
import { useStore } from "../store";
import { editedDuration } from "../lib/cuts";
import { formatTime } from "../lib/format";
import type { EditMode } from "../lib/types";
import { Button, IconButton, cx } from "./ui";

const IS_MAC = navigator.userAgent.includes("Mac");

export function Logo() {
  return (
    <div className="flex items-center gap-2">
      {/* Same mark as the app icon (assets/logo.svg), cropped to the tile. */}
      <svg viewBox="100 100 824 824" className="h-6 w-6" aria-hidden>
        <defs>
          <linearGradient id="nu-logo-bg" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stopColor="#8f80ff" />
            <stop offset="0.55" stopColor="#6a58ff" />
            <stop offset="1" stopColor="#4434d4" />
          </linearGradient>
        </defs>
        <rect x="100" y="100" width="824" height="824" rx="185" fill="url(#nu-logo-bg)" />
        <g fill="none" stroke="#fff" strokeWidth="96" strokeLinecap="round" strokeLinejoin="round">
          <path d="M236 646 V378 L436 646 V378" />
          <path d="M588 378 V546 A100 100 0 0 0 788 546 V378" />
        </g>
      </svg>
      <span className="text-[15px] font-semibold tracking-tight">nuai</span>
    </div>
  );
}

export function Header({ onOpen }: { onOpen: () => void }) {
  const { t } = useTranslation();
  const media = useStore((s) => s.media);
  const project = useStore((s) => s.project);
  const mode = useStore((s) => s.settings?.mode ?? "review");
  const busy = useStore((s) => s.busy);
  const updateSettings = useStore((s) => s.updateSettings);
  const openSettings = useStore((s) => s.openSettings);
  const openExport = useStore((s) => s.openExport);

  const edited = useMemo(
    () => (project && media ? editedDuration(project.cuts, media.duration) : null),
    [project, media],
  );

  const setMode = (m: EditMode) => updateSettings((s) => ({ ...s, mode: m }));

  return (
    <header
      data-tauri-drag-region
      className={cx(
        "flex h-12 shrink-0 items-center gap-4 border-b border-line bg-panel pr-3",
        // Room for the macOS traffic lights (the title bar is overlaid on macOS).
        IS_MAC ? "pl-[84px]" : "pl-3",
      )}
    >
      <Logo />
      {media && (
        <span className="max-w-[320px] truncate text-muted" title={media.path}>
          {media.fileName}
        </span>
      )}

      <div className="mx-auto flex rounded-lg border border-line bg-bg p-0.5" data-tauri-drag-region>
        {(["review", "auto"] as const).map((m) => (
          <button
            key={m}
            type="button"
            title={t(m === "review" ? "header.reviewHint" : "header.autoHint")}
            onClick={() => setMode(m)}
            className={cx(
              "rounded-md px-3 py-1 text-xs font-medium transition-colors",
              mode === m ? "bg-raised text-fg shadow" : "text-muted hover:text-fg",
            )}
          >
            {t(m === "review" ? "header.review" : "header.auto")}
          </button>
        ))}
      </div>

      {media && edited !== null && (
        <div className="flex items-center gap-2 font-mono text-xs">
          <span className="text-muted">{formatTime(media.duration)}</span>
          <span className="text-faint">→</span>
          <span className="text-fg">{formatTime(edited)}</span>
          {media.duration > 0 && edited < media.duration && (
            <span className="rounded bg-ok/15 px-1.5 py-0.5 font-sans text-[11px] font-medium text-ok">
              {t("header.shorter", { pct: Math.round((1 - edited / media.duration) * 100) })}
            </span>
          )}
        </div>
      )}

      <div className="flex items-center gap-1.5">
        <IconButton label={t("header.settings")} onClick={() => openSettings()}>
          <SettingsIcon size={16} />
        </IconButton>
        <Button onClick={onOpen} disabled={!!busy}>
          <FolderOpen size={14} />
          {t("header.open")}
        </Button>
        <Button variant="primary" onClick={() => void openExport()} disabled={!project || !!busy}>
          <Upload size={14} />
          {t("header.export")}
        </Button>
      </div>
    </header>
  );
}
