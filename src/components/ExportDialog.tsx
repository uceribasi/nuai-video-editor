import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { CheckCircle2, FolderOpen, RefreshCw, TriangleAlert, Upload } from "lucide-react";
import { useStore, subtitlesStale } from "../store";
import { api } from "../lib/api";
import { keepSegments } from "../lib/cuts";
import { formatTime } from "../lib/format";
import type { ExportSettings } from "../lib/types";
import { Button, Checkbox, Modal, ProgressBar, Select, TextInput, cx } from "./ui";

export function ExportDialog() {
  const { t } = useTranslation();
  const state = useStore((s) => s.exportState);
  const media = useStore((s) => s.media);
  const project = useStore((s) => s.project);
  const close = useStore((s) => s.closeExport);
  const setOutput = useStore((s) => s.setExportOutput);
  const runExport = useStore((s) => s.runExport);
  const settings = useStore((s) => s.settings);
  const updateSettings = useStore((s) => s.updateSettings);
  const generateSubtitles = useStore((s) => s.generateSubtitles);
  const setBurnLanguage = useStore((s) => s.setBurnLanguage);
  const setExportAudio = useStore((s) => s.setExportAudio);

  const keeps = useMemo(
    () => (project && media ? keepSegments(project.cuts, media.duration) : []),
    [project, media],
  );
  const edited = keeps.reduce((s, [a, b]) => s + (b - a), 0);
  if (!media || !project || !settings) return null;
  const ex = settings.export;
  const setEx = (patch: Partial<ExportSettings>) => updateSettings((s) => ({ ...s, export: { ...s.export, ...patch } }));
  const tracks = project.subtitles?.tracks ?? [];
  const stale = subtitlesStale(project, media.duration);
  const wantsSubs = ex.subtitleMode !== "none" || ex.subtitleSidecar;
  const dubs = project.dubs ?? {};
  const exportDub = state.audioLanguage && dubs[state.audioLanguage] ? state.audioLanguage : (Object.keys(dubs)[0] ?? "");

  const pick = async () => {
    const p = await save({ defaultPath: state.output, filters: [{ name: "MP4", extensions: ["mp4"] }] });
    if (p) setOutput(p.toLowerCase().endsWith(".mp4") ? p : `${p}.mp4`);
  };

  const footer = state.result ? (
    <>
      <Button onClick={() => void revealItemInDir(state.result!.output)}>
        <FolderOpen size={14} />
        {t("export.reveal")}
      </Button>
      <Button variant="primary" onClick={close}>
        {t("common.close")}
      </Button>
    </>
  ) : state.running ? (
    <Button onClick={() => void api.cancelTask()}>{t("common.cancel")}</Button>
  ) : (
    <>
      <Button onClick={close}>{t("common.cancel")}</Button>
      <Button variant="primary" onClick={() => void runExport()} disabled={!state.output || keeps.length === 0}>
        <Upload size={14} />
        {t("export.start")}
      </Button>
    </>
  );

  return (
    <Modal title={t("export.title")} onClose={close} footer={footer} width={520}>
      <div className="flex flex-col gap-4 p-5">
        <div className="flex items-baseline justify-between rounded-lg bg-raised/60 px-4 py-3">
          <span className="font-mono text-base">
            {t("export.summary", { from: formatTime(media.duration), to: formatTime(edited) })}
          </span>
          <span className="text-xs text-muted">{t("export.segments", { count: keeps.length })}</span>
        </div>

        {!state.result && (
          <label className="flex flex-col gap-1.5">
            <span className="text-xs font-medium text-muted">{t("export.output")}</span>
            <div className="flex gap-2">
              <TextInput readOnly value={state.output} className="font-mono text-xs" title={state.output} />
              <Button onClick={() => void pick()} disabled={state.running}>
                {t("export.change")}
              </Button>
            </div>
          </label>
        )}

        {!state.result && Object.keys(project.dubs ?? {}).length > 0 && (
          <div className="flex flex-col gap-2">
            <span className="text-xs font-medium text-muted">{t("export.audio")}</span>
            <div className="grid grid-cols-3 gap-1 rounded-lg border border-line bg-bg p-1">
              {(["original", "replace", "add"] as const).map((m) => (
                <button
                  key={m}
                  type="button"
                  disabled={state.running}
                  onClick={() => setExportAudio(m)}
                  className={cx(
                    "rounded-md px-2 py-1.5 text-xs font-medium",
                    state.audioMode === m ? "bg-raised text-fg shadow" : "text-muted hover:text-fg",
                  )}
                >
                  {t(`export.audioMode.${m}`)}
                </button>
              ))}
            </div>
            <p className="text-xs text-faint">
              {t(`export.audioModeHint.${state.audioMode}`, { language: t(`languages.${exportDub}`, { defaultValue: exportDub }) })}
            </p>
            {state.audioMode === "replace" && Object.keys(project.dubs ?? {}).length > 1 && (
              <Select
                value={state.audioLanguage ?? Object.keys(project.dubs ?? {})[0]}
                onChange={(v) => setExportAudio("replace", v)}
                options={Object.keys(project.dubs ?? {}).map((l) => ({ value: l, label: t(`languages.${l}`, { defaultValue: l }) }))}
              />
            )}
          </div>
        )}

        {!state.result && project.transcript && (
          <div className="flex flex-col gap-2">
            <span className="text-xs font-medium text-muted">{t("export.subtitles")}</span>
            <div className="grid grid-cols-3 gap-1 rounded-lg border border-line bg-bg p-1">
              {(["none", "embed", "burn"] as const).map((m) => (
                <button
                  key={m}
                  type="button"
                  disabled={state.running || (m === "burn" && !media.hasVideo)}
                  title={t(`export.subtitleModeHint.${m}`)}
                  onClick={() => setEx({ subtitleMode: m })}
                  className={cx(
                    "rounded-md px-2 py-1.5 text-xs font-medium disabled:opacity-40",
                    ex.subtitleMode === m ? "bg-raised text-fg shadow" : "text-muted hover:text-fg",
                  )}
                >
                  {t(`export.subtitleMode.${m}`)}
                </button>
              ))}
            </div>
            {ex.subtitleMode !== "none" && (
              <p className="text-xs text-faint">{t(`export.subtitleModeHint.${ex.subtitleMode}`)}</p>
            )}
            {ex.subtitleMode === "burn" && tracks.length > 1 && (
              <Select
                value={state.burnLanguage ?? tracks[0].language}
                onChange={setBurnLanguage}
                options={tracks.map((x) => ({ value: x.language, label: t(`languages.${x.language}`, { defaultValue: x.language }) }))}
              />
            )}
            <label className="flex items-center gap-2 text-xs text-muted">
              <Checkbox checked={ex.subtitleSidecar} onChange={(v) => setEx({ subtitleSidecar: v })} />
              {t("export.sidecar")}
            </label>
            {wantsSubs && !tracks.length && <p className="text-xs text-faint">{t("export.subtitlesAuto")}</p>}
            {wantsSubs && stale && (
              <div className="flex items-center gap-2 rounded-md bg-warn/10 p-2 text-xs text-warn">
                <TriangleAlert size={13} className="shrink-0" />
                <span className="flex-1">{t("subtitles.stale")}</span>
                <Button size="sm" onClick={() => void generateSubtitles()} disabled={state.running}>
                  <RefreshCw size={12} />
                  {t("subtitles.regenerate")}
                </Button>
              </div>
            )}
          </div>
        )}

        {state.running && (
          <div className="flex flex-col gap-2">
            <div className="flex justify-between text-xs text-muted">
              <span>{t(`stages.${state.stage}`)}…</span>
              <span className="font-mono">{Math.round(state.progress * 100)}%</span>
            </div>
            <ProgressBar value={state.progress} />
          </div>
        )}

        {state.result && (
          <div className="flex items-start gap-3 rounded-lg border border-ok/30 bg-ok/10 p-3">
            <CheckCircle2 size={18} className="mt-0.5 shrink-0 text-ok" />
            <div className="min-w-0">
              <div className="font-medium">{t("export.done")}</div>
              <div className="selectable truncate font-mono text-xs text-muted" title={state.result.output}>
                {state.result.output}
              </div>
              {state.result.subtitleFiles.map((f) => (
                <div key={f} className="selectable truncate font-mono text-xs text-muted" title={f}>
                  {f}
                </div>
              ))}
            </div>
          </div>
        )}

        {state.error && (
          <div className="flex items-start gap-3 rounded-lg border border-danger/30 bg-danger/10 p-3">
            <TriangleAlert size={18} className="mt-0.5 shrink-0 text-danger" />
            <div className="min-w-0">
              <div className="font-medium">{t("export.failed")}</div>
              <div className="selectable whitespace-pre-wrap break-words font-mono text-xs text-muted">{state.error}</div>
            </div>
          </div>
        )}
      </div>
    </Modal>
  );
}
