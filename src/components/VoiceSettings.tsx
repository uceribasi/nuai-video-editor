import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { AudioLines, CheckCircle2, Download, Loader2, Mic, Play, ShieldCheck, Trash2, X } from "lucide-react";
import { useStore } from "../store";
import { api, errorText, isCancelled } from "../lib/api";
import { formatBytes } from "../lib/format";
import type { VoiceStatus } from "../lib/types";
import { Button, Checkbox, ProgressBar, Select, cx } from "./ui";
import { SUBTITLE_LANGUAGES } from "./SubtitlePanel";

const INSTALL_STAGES = ["runtime", "packages", "models"] as const;

function InstallProgress({ stage, progress }: { stage: string; progress: number }) {
  const { t } = useTranslation();
  const current = INSTALL_STAGES.indexOf(stage as (typeof INSTALL_STAGES)[number]);
  return (
    <div className="flex flex-col gap-2">
      {INSTALL_STAGES.map((s, i) => {
        const done = i < current;
        const running = i === current;
        return (
          <div key={s} className="flex flex-col gap-1">
            <div className={cx("flex items-center gap-2 text-[13px]", !done && !running && "text-faint")}>
              {done ? (
                <CheckCircle2 size={14} className="text-ok" />
              ) : running ? (
                <Loader2 size={14} className="animate-spin text-accent" />
              ) : (
                <span className="inline-block h-3.5 w-3.5 rounded-full border border-line-strong" />
              )}
              <span className="flex-1">{t(`voice.stage.${s}`)}</span>
              {running && progress >= 0 && <span className="font-mono text-xs text-muted">{Math.round(progress * 100)}%</span>}
            </div>
            {running && <ProgressBar value={progress >= 0 ? progress : 0.05} />}
          </div>
        );
      })}
    </div>
  );
}

export function VoiceSettings() {
  const { t } = useTranslation();
  const progress = useStore((s) => s.voiceProgress);
  const [status, setStatus] = useState<VoiceStatus | null>(null);
  const [busy, setBusy] = useState<"install" | "try" | "remove" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [consent, setConsent] = useState(false);
  const [sample, setSample] = useState("");
  const [text, setText] = useState("");
  const [language, setLanguage] = useState("en");
  const [result, setResult] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement>(null);

  useEffect(() => {
    void api.voiceStatus().then(setStatus);
  }, []);

  const run = async (kind: "install" | "remove") => {
    setBusy(kind);
    setError(null);
    useStore.setState({ voiceProgress: null });
    try {
      setStatus(kind === "install" ? await api.voiceInstall() : await api.voiceUninstall());
    } catch (e) {
      if (!isCancelled(e)) setError(errorText(e));
      setStatus(await api.voiceStatus());
    } finally {
      setBusy(null);
    }
  };

  const tryVoice = async () => {
    setBusy("try");
    setError(null);
    setResult(null);
    useStore.setState({ voiceProgress: null });
    try {
      const out = await api.voiceTry(sample, text, language);
      setResult(out);
      requestAnimationFrame(() => void audioRef.current?.play());
    } catch (e) {
      if (!isCancelled(e)) setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  if (!status) return null;
  const installing = busy === "install";

  return (
    <div className="flex flex-col gap-5">
      <div className="flex gap-3 rounded-lg border border-line bg-raised/40 p-4">
        <Mic size={20} className="mt-0.5 shrink-0 text-accent" />
        <div className="flex flex-col gap-1">
          <div className="font-medium">{t("voice.title")}</div>
          <p className="text-xs text-muted">{t("voice.description")}</p>
        </div>
      </div>

      <div className="flex flex-col gap-3 rounded-lg border border-line p-4">
        <div className="flex items-center gap-3">
          <span className="flex-1 font-medium">{t("voice.engine")}</span>
          {status.installed ? (
            <span className="flex items-center gap-1 text-xs text-ok">
              <CheckCircle2 size={13} />
              {t("voice.ready")}
            </span>
          ) : (
            <span className="text-xs text-muted">
              {t("voice.notInstalled", { size: formatBytes(status.downloadMb * 1e6) })}
            </span>
          )}
        </div>
        {installing && progress && <InstallProgress stage={progress.stage} progress={progress.progress} />}
        {installing && !progress && <ProgressBar value={0.03} />}
        <div className="flex gap-2">
          {installing ? (
            <Button onClick={() => void api.voiceCancel()}>
              <X size={13} />
              {t("common.cancel")}
            </Button>
          ) : status.installed ? (
            <Button variant="ghost" onClick={() => void run("remove")} disabled={!!busy}>
              <Trash2 size={13} />
              {t("voice.remove", { size: formatBytes(status.totalMb * 1e6) })}
            </Button>
          ) : (
            <Button variant="primary" onClick={() => void run("install")} disabled={!!busy}>
              <Download size={14} />
              {t("voice.install", { size: formatBytes(status.downloadMb * 1e6) })}
            </Button>
          )}
        </div>
        <p className="text-xs text-faint">{t("voice.installNote")}</p>
      </div>

      {status.installed && (
        <div className="flex flex-col gap-3 rounded-lg border border-line p-4">
          <div className="font-medium">{t("voice.tryTitle")}</div>
          <label className="flex items-start gap-2 text-xs text-muted">
            <span className="pt-0.5">
              <Checkbox checked={consent} onChange={setConsent} />
            </span>
            <span className="flex items-start gap-1.5">
              <ShieldCheck size={13} className="mt-px shrink-0 text-ok" />
              {t("voice.consent")}
            </span>
          </label>
          <div className="flex gap-2">
            <div className="flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md border border-line bg-bg px-2.5 text-xs text-muted">
              <AudioLines size={13} className="shrink-0" />
              <span className="truncate" title={sample}>
                {sample ? sample.split(/[\\/]/).pop() : t("voice.noSample")}
              </span>
            </div>
            <Button
              onClick={async () => {
                const p = await openDialog({
                  multiple: false,
                  filters: [{ name: "Audio / Video", extensions: ["mp3", "wav", "m4a", "aac", "flac", "ogg", "mp4", "mov", "mkv", "webm"] }],
                });
                if (typeof p === "string") setSample(p);
              }}
            >
              {t("common.browse")}
            </Button>
          </div>
          <p className="-mt-1 text-xs text-faint">{t("voice.sampleHint")}</p>
          <textarea
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={t("voice.textPlaceholder")}
            rows={3}
            className="w-full resize-none rounded-md border border-line bg-bg px-2.5 py-2 text-fg placeholder:text-faint focus:border-accent focus:outline-none"
          />
          <div className="flex items-center gap-2">
            <Select
              className="w-44"
              value={language}
              onChange={setLanguage}
              options={SUBTITLE_LANGUAGES.map((l) => ({ value: l, label: t(`languages.${l}`, { defaultValue: l }) }))}
            />
            <Button
              variant="primary"
              disabled={!consent || !sample || !text.trim() || !!busy}
              onClick={() => void tryVoice()}
            >
              {busy === "try" ? <Loader2 size={13} className="animate-spin" /> : <Play size={13} />}
              {t("voice.speak")}
            </Button>
            {busy === "try" && (
              <>
                <span className="text-xs text-muted">
                  {t(`voice.step.${progress?.stage ?? "audio"}`, { defaultValue: progress?.stage ?? "" })}…
                </span>
                <button type="button" className="ml-auto text-xs text-faint hover:text-fg" onClick={() => void api.voiceCancel()}>
                  {t("common.cancel")}
                </button>
              </>
            )}
          </div>
          {result && <audio ref={audioRef} src={convertFileSrc(result)} controls className="w-full" />}
        </div>
      )}

      {error && <p className="selectable whitespace-pre-wrap text-xs text-danger">{error}</p>}
    </div>
  );
}
