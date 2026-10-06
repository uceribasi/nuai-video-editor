import { useEffect, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { AudioLines, Download, Image, MessageSquareText, Sparkles, Wand2 } from "lucide-react";
import { useStore } from "../store";
import { formatTime, parseTime } from "../lib/format";
import { Button, ProgressBar, Select, Slider, TextInput, Toggle, cx } from "./ui";
import { StageList } from "./StageList";

const LANGUAGES = ["auto", "tr", "en", "de", "fr", "es", "it", "pt", "nl", "pl", "ru", "uk", "ar", "fa", "hi", "ja", "ko", "zh"];

function Section({
  icon,
  title,
  description,
  enabled,
  onToggle,
  toggleDisabled,
  children,
}: {
  icon: ReactNode;
  title: string;
  description?: string;
  enabled: boolean;
  onToggle: (v: boolean) => void;
  toggleDisabled?: boolean;
  children?: ReactNode;
}) {
  return (
    <section className="rounded-lg border border-line bg-raised/40">
      <div className="flex items-start gap-2.5 p-3">
        <div className={cx("mt-0.5", enabled ? "text-accent" : "text-faint")}>{icon}</div>
        <div className="min-w-0 flex-1">
          <div className="font-medium">{title}</div>
          {description && <div className="mt-0.5 text-xs text-muted">{description}</div>}
        </div>
        <Toggle checked={enabled} onChange={onToggle} disabled={toggleDisabled} label={title} />
      </div>
      {children && enabled && <div className="flex flex-col gap-3 border-t border-line px-3 py-3">{children}</div>}
    </section>
  );
}

function SubToggle({ label, checked, onChange }: { label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="flex items-center justify-between gap-3 text-[13px]">
      <span className="text-fg/90">{label}</span>
      <Toggle checked={checked} onChange={onChange} label={label} />
    </label>
  );
}

function TargetInput({ value, onChange }: { value: number; onChange: (v: number) => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState(value > 0 ? formatTime(value) : "");
  useEffect(() => setText(value > 0 ? formatTime(value) : ""), [value]);
  return (
    <label className="flex flex-col gap-1">
      <span className="text-xs text-muted">{t("analyze.target")}</span>
      <TextInput
        value={text}
        placeholder={t("analyze.targetPlaceholder")}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          const secs = text.trim() ? parseTime(text) : 0;
          onChange(Number.isFinite(secs) ? secs : 0);
        }}
        className="font-mono"
      />
    </label>
  );
}

export function AnalyzePanel() {
  const { t } = useTranslation();
  const settings = useStore((s) => s.settings);
  const status = useStore((s) => s.status);
  const project = useStore((s) => s.project);
  const media = useStore((s) => s.media);
  const busy = useStore((s) => s.busy);
  const downloads = useStore((s) => s.downloads);
  const updateOptions = useStore((s) => s.updateOptions);
  const updateSettings = useStore((s) => s.updateSettings);
  const analyze = useStore((s) => s.analyze);
  const cancel = useStore((s) => s.cancel);
  const openSettings = useStore((s) => s.openSettings);
  const downloadModel = useStore((s) => s.downloadModel);

  if (!settings) return null;
  const o = settings.analysis;
  const aiConfigured = settings.ai.provider !== "none";
  const hasManualEdits = project?.cuts.some((c) => c.source === "user") ?? false;
  const modelId = settings.transcription.model;
  const modelReady =
    !!settings.transcription.customModelPath.trim() || !!status?.models.find((m) => m.id === modelId)?.installed;
  const speechWanted = o.transcribe && (o.removeRetakes || o.removeFillers || o.removeStutters || o.useAi);
  const blocked = speechWanted && !modelReady;
  const analyzing = busy?.task === "analyze";

  return (
    <aside className="flex w-[300px] shrink-0 flex-col border-r border-line bg-panel">
      <div className="flex items-center gap-2 px-4 pb-2 pt-3 text-xs font-medium uppercase tracking-wide text-faint">
        <Wand2 size={13} />
        {t("analyze.title")}
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-2.5 overflow-y-auto px-3 pb-3">
        <Section
          icon={<AudioLines size={16} />}
          title={t("analyze.silence")}
          description={t("analyze.silenceDesc")}
          enabled={o.removeSilence}
          onToggle={(v) => updateOptions({ removeSilence: v })}
          toggleDisabled={media ? !media.hasAudio : false}
        >
          <Slider
            label={t("analyze.minSilence")}
            value={o.minSilence}
            min={0.2}
            max={3}
            step={0.1}
            format={(v) => `${v.toFixed(1)} ${t("common.seconds")}`}
            onChange={(v) => updateOptions({ minSilence: v })}
          />
          <Slider
            label={t("analyze.padding")}
            value={o.padding}
            min={0}
            max={0.5}
            step={0.01}
            format={(v) => `${v.toFixed(2)} ${t("common.seconds")}`}
            onChange={(v) => updateOptions({ padding: v })}
          />
          <Slider
            label={t("analyze.sensitivity")}
            value={o.sensitivity}
            min={-12}
            max={12}
            step={1}
            format={(v) => `${v > 0 ? "+" : ""}${v} dB`}
            onChange={(v) => updateOptions({ sensitivity: v })}
          />
        </Section>

        <Section
          icon={<MessageSquareText size={16} />}
          title={t("analyze.transcribe")}
          description={t("analyze.transcribeDesc")}
          enabled={o.transcribe}
          onToggle={(v) => updateOptions({ transcribe: v })}
          toggleDisabled={media ? !media.hasAudio : false}
        >
          <Select
            value={settings.transcription.language}
            onChange={(v) => updateSettings((s) => ({ ...s, transcription: { ...s.transcription, language: v } }))}
            options={LANGUAGES.map((l) => ({ value: l, label: t(`languages.${l}`) }))}
          />
          <SubToggle label={t("analyze.retakes")} checked={o.removeRetakes} onChange={(v) => updateOptions({ removeRetakes: v })} />
          <SubToggle label={t("analyze.fillers")} checked={o.removeFillers} onChange={(v) => updateOptions({ removeFillers: v })} />
          <SubToggle label={t("analyze.stutters")} checked={o.removeStutters} onChange={(v) => updateOptions({ removeStutters: v })} />
          {!modelReady && (
            <div className="flex flex-col gap-2 rounded-md bg-warn/10 p-2.5 text-xs text-warn">
              {t("analyze.needModel")}
              {downloads[modelId] !== undefined ? (
                <ProgressBar value={downloads[modelId]} />
              ) : (
                <Button size="sm" onClick={() => void downloadModel(modelId)} className="self-start">
                  <Download size={13} />
                  {t("settings.transcription.download")} · {modelId}
                </Button>
              )}
            </div>
          )}
        </Section>

        <Section
          icon={<Image size={16} />}
          title={t("analyze.static")}
          description={t("analyze.staticDesc")}
          enabled={o.removeStatic}
          onToggle={(v) => updateOptions({ removeStatic: v })}
          toggleDisabled={media ? !media.hasVideo : false}
        >
          <Slider
            label={t("analyze.minStatic")}
            value={o.minStatic}
            min={1}
            max={10}
            step={0.5}
            format={(v) => `${v.toFixed(1)} ${t("common.seconds")}`}
            onChange={(v) => updateOptions({ minStatic: v })}
          />
        </Section>

        <Section
          icon={<Sparkles size={16} />}
          title={t("analyze.ai")}
          description={aiConfigured ? t("analyze.aiDesc") : t("analyze.aiNotConfigured")}
          enabled={o.useAi && aiConfigured}
          onToggle={(v) => (aiConfigured ? updateOptions({ useAi: v }) : openSettings("ai"))}
        >
          <label className="flex flex-col gap-1">
            <span className="text-xs text-muted">{t("analyze.instructions")}</span>
            <textarea
              value={o.instructions}
              onChange={(e) => updateOptions({ instructions: e.target.value })}
              placeholder={t("analyze.instructionsPlaceholder")}
              rows={3}
              className="w-full resize-none rounded-md border border-line bg-bg px-2.5 py-2 text-fg placeholder:text-faint focus:border-accent focus:outline-none"
            />
          </label>
          <TargetInput value={o.targetDuration} onChange={(v) => updateOptions({ targetDuration: v })} />
        </Section>
      </div>

      <div className="border-t border-line p-3">
        {analyzing && busy ? (
          <div className="flex flex-col gap-3">
            <StageList busy={busy} />
            <Button onClick={cancel}>{t("common.cancel")}</Button>
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            {project && hasManualEdits && <p className="text-xs text-muted">{t("analyze.rerunWarning")}</p>}
            <Button
              variant="primary"
              size="lg"
              disabled={!media || !!busy || blocked}
              onClick={() => void analyze()}
              title={blocked ? t("analyze.needModel") : undefined}
            >
              <Wand2 size={15} />
              {settings.mode === "auto" ? t("analyze.runAuto") : project ? t("analyze.rerun") : t("analyze.run")}
            </Button>
          </div>
        )}
      </div>
    </aside>
  );
}
