import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { save } from "@tauri-apps/plugin-dialog";
import { Captions, ChevronDown, ChevronRight, Download, Headphones, Languages, Loader2, Mic, RefreshCw, TriangleAlert, X } from "lucide-react";
import { useStore, subtitlesStale } from "../store";
import { api, errorText } from "../lib/api";
import { formatTime } from "../lib/format";
import { player } from "../lib/player";
import type { CaptionSize, CaptionStyle, Cue, SubtitleOptions, SubtitleTrack } from "../lib/types";
import { Button, Checkbox, ProgressBar, Select, Slider, Toggle, cx } from "./ui";

/** Speak a subtitle track with the voice of the person in the video. */
function DubSection({ track }: { track: SubtitleTrack }) {
  const { t } = useTranslation();
  const voice = useStore((s) => s.voice);
  const project = useStore((s) => s.project);
  const dubbing = useStore((s) => s.dubbing);
  const audioSource = useStore((s) => s.audioSource);
  const createDub = useStore((s) => s.createDub);
  const cancelDub = useStore((s) => s.cancelDub);
  const setVoiceConsent = useStore((s) => s.setVoiceConsent);
  const setAudioSource = useStore((s) => s.setAudioSource);
  const openSettings = useStore((s) => s.openSettings);
  const name = t(`languages.${track.language}`, { defaultValue: track.language });

  if (!voice?.installed) {
    return (
      <div className="flex items-center gap-2 text-xs text-muted">
        <Mic size={14} className="shrink-0 text-faint" />
        <span className="flex-1">{t("dub.needEngine")}</span>
        <Button size="sm" onClick={() => openSettings("voice")}>
          {t("dub.setup")}
        </Button>
      </div>
    );
  }
  if (dubbing) {
    const mine = dubbing.language === track.language;
    return (
      <div className="flex flex-col gap-1.5">
        <div className="flex items-center gap-2 text-xs text-muted">
          <Loader2 size={13} className="animate-spin text-accent" />
          <span className="flex-1">
            {t(`dub.stage.${dubbing.stage}`, { defaultValue: dubbing.stage, language: t(`languages.${dubbing.language}`, { defaultValue: dubbing.language }) })}
            {!mine && ` · ${t(`languages.${dubbing.language}`, { defaultValue: dubbing.language })}`}
          </span>
          <button type="button" className="text-faint hover:text-fg" onClick={cancelDub}>
            {t("common.cancel")}
          </button>
        </div>
        <ProgressBar value={Math.max(0.03, dubbing.progress)} />
      </div>
    );
  }
  const done = !!project?.dubs?.[track.language];
  return (
    <div className="flex flex-col gap-2">
      {!project?.voiceConsent && (
        <label className="flex items-start gap-2 text-xs text-muted">
          <span className="pt-0.5">
            <Checkbox checked={false} onChange={setVoiceConsent} />
          </span>
          {t("dub.consent")}
        </label>
      )}
      <div className="flex items-center gap-2">
        <Button onClick={() => void createDub(track.language)} disabled={!project?.voiceConsent} className="flex-1">
          <Mic size={14} />
          {done ? t("dub.redo", { language: name }) : t("dub.create", { language: name })}
        </Button>
        {done && (
          <Button
            variant={audioSource === track.language ? "primary" : "secondary"}
            title={t("dub.listen")}
            onClick={() => setAudioSource(audioSource === track.language ? "original" : track.language)}
          >
            <Headphones size={14} />
          </Button>
        )}
      </div>
      <p className="text-[11px] text-faint">{done ? t("dub.ready") : t("dub.hint")}</p>
    </div>
  );
}

/** Languages offered for translation (kept in sync with `subtitles.rs`). */
export const SUBTITLE_LANGUAGES = [
  "tr", "en", "de", "fr", "es", "it", "pt", "nl", "pl", "ru", "uk", "ar", "fa", "hi",
  "ja", "ko", "zh", "az", "el", "he", "id", "ro", "sv", "cs", "hu", "vi",
];

function OptionsEditor({ value, onChange }: { value: SubtitleOptions; onChange: (o: SubtitleOptions) => void }) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-3">
      <Slider
        label={t("subtitles.maxChars")}
        value={value.maxChars}
        min={16}
        max={60}
        step={1}
        format={(v) => t("subtitles.chars", { count: v })}
        onChange={(v) => onChange({ ...value, maxChars: v })}
      />
      <div className="flex items-center justify-between text-xs text-muted">
        <span>{t("subtitles.lines")}</span>
        <div className="flex rounded-md border border-line bg-bg p-0.5">
          {[1, 2].map((n) => (
            <button
              key={n}
              type="button"
              onClick={() => onChange({ ...value, maxLines: n })}
              className={cx("rounded px-2.5 py-0.5", value.maxLines === n ? "bg-raised text-fg" : "text-muted hover:text-fg")}
            >
              {n}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

function CueRow({ cue, language, onSeek }: { cue: Cue; language: string; onSeek: () => void }) {
  const updateCue = useStore((s) => s.updateCue);
  const [text, setText] = useState(cue.text);
  useEffect(() => setText(cue.text), [cue.text]);
  return (
    <div data-cue={cue.id} className="cue-row group flex gap-2 rounded-md px-2 py-1.5 hover:bg-hover">
      <button
        type="button"
        onClick={onSeek}
        className="mt-1 h-fit shrink-0 font-mono text-[10.5px] text-faint hover:text-accent"
      >
        {formatTime(cue.start, true)}
      </button>
      <textarea
        value={text}
        rows={Math.max(1, text.split("\n").length)}
        spellCheck={false}
        onChange={(e) => setText(e.target.value)}
        onFocus={onSeek}
        onBlur={() => text !== cue.text && updateCue(language, cue.id, text)}
        className="w-full resize-none rounded border border-transparent bg-transparent px-1.5 py-0.5 text-[13px] leading-snug text-fg/90 focus:border-line focus:bg-bg focus:outline-none"
      />
    </div>
  );
}

export function SubtitlePanel() {
  const { t } = useTranslation();
  const project = useStore((s) => s.project);
  const media = useStore((s) => s.media);
  const settings = useStore((s) => s.settings);
  const lang = useStore((s) => s.subtitleLang);
  const translating = useStore((s) => s.translating);
  const notice = useStore((s) => s.subtitleNotice);
  const showCaptions = useStore((s) => s.showCaptions);
  const updateSettings = useStore((s) => s.updateSettings);
  const generate = useStore((s) => s.generateSubtitles);
  const translate = useStore((s) => s.translateSubtitles);
  const cancelTranslation = useStore((s) => s.cancelTranslation);
  const removeTrack = useStore((s) => s.removeTrack);
  const selectTrack = useStore((s) => s.selectSubtitleTrack);
  const setShowCaptions = useStore((s) => s.setShowCaptions);
  const openSettings = useStore((s) => s.openSettings);
  const [target, setTarget] = useState("");
  const [showOptions, setShowOptions] = useState(false);
  const [showNote, setShowNote] = useState(false);
  const [saved, setSaved] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const tracks = project?.subtitles?.tracks ?? [];
  const track = tracks.find((x) => x.language === lang) ?? tracks[0];
  const stale = !!media && subtitlesStale(project, media.duration);
  const available = useMemo(
    () => SUBTITLE_LANGUAGES.filter((l) => !tracks.some((x) => x.language === l)),
    [tracks],
  );
  useEffect(() => {
    if (!available.includes(target)) setTarget(available.find((l) => l === "en") ?? available[0] ?? "");
  }, [available, target]);

  // Highlight the cue under the playhead.
  useEffect(() => {
    if (!track) return;
    let raf = 0;
    let current = -1;
    const loop = () => {
      const now = player.el?.currentTime ?? 0;
      const cue = track.cues.find((c) => now >= c.start && now < c.end);
      const id = cue?.id ?? -1;
      if (id !== current) {
        const root = listRef.current;
        root?.querySelector(".cue-row.bg-accent-soft")?.classList.remove("bg-accent-soft");
        if (root && cue) {
          const el = root.querySelector(`[data-cue="${id}"]`);
          el?.classList.add("bg-accent-soft");
          if (el && player.el && !player.el.paused) el.scrollIntoView({ block: "nearest" });
        }
        current = id;
      }
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [track]);

  if (!settings) return null;
  const sub = settings.subtitles;
  const setSub = (patch: Partial<typeof sub>) => updateSettings((s) => ({ ...s, subtitles: { ...s.subtitles, ...patch } }));

  if (!project?.transcript) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center text-muted">
        <Captions size={28} className="text-faint" />
        {project ? t("subtitles.needTranscript") : t("subtitles.needAnalysis")}
      </div>
    );
  }

  if (!track) {
    return (
      <div className="flex flex-1 flex-col gap-4 overflow-y-auto p-4">
        <div className="flex flex-col items-center gap-2 pt-4 text-center">
          <Captions size={28} className="text-accent" />
          <p className="text-muted">{t("subtitles.intro")}</p>
        </div>
        <OptionsEditor value={sub.options} onChange={(o) => setSub({ options: o })} />
        <Button variant="primary" size="lg" onClick={() => void generate()}>
          <Captions size={15} />
          {t("subtitles.generate")}
        </Button>
      </div>
    );
  }

  const saveFile = async (format: "srt" | "vtt") => {
    if (!media) return;
    const def = await api.defaultSubtitlePath(media.path, track.language);
    const path = await save({
      defaultPath: format === "vtt" ? def.replace(/\.srt$/, ".vtt") : def,
      filters: [{ name: format.toUpperCase(), extensions: [format] }],
    });
    if (!path) return;
    try {
      await api.exportSubtitles(media.path, project.cuts, track, path);
      setSaved(path);
    } catch (e) {
      useStore.setState({ error: errorText(e) });
    }
  };

  const aiReady = settings.ai.provider !== "none";

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-col gap-2.5 border-b border-line p-3">
        {stale && (
          <div className="flex items-start gap-2 rounded-md bg-warn/10 p-2.5 text-xs text-warn">
            <TriangleAlert size={14} className="mt-0.5 shrink-0" />
            <div className="flex-1">
              {t("subtitles.stale")}
              {tracks.length > 1 && <span className="text-warn/80"> {t("subtitles.staleTranslations")}</span>}
            </div>
            <Button size="sm" onClick={() => void generate()}>
              <RefreshCw size={12} />
              {t("subtitles.regenerate")}
            </Button>
          </div>
        )}

        <div className="flex flex-wrap gap-1.5">
          {tracks.map((x) => (
            <span
              key={x.language}
              className={cx(
                "inline-flex items-center gap-1 rounded-full border py-0.5 pl-2.5 text-xs",
                x.translated ? "pr-1" : "pr-2.5",
                x.language === track.language ? "border-accent bg-accent-soft text-fg" : "border-line text-muted hover:text-fg",
              )}
            >
              <button type="button" onClick={() => selectTrack(x.language)}>
                {t(`languages.${x.language}`, { defaultValue: x.language })}
                {!x.translated && <span className="ml-1 text-faint">· {t("subtitles.original")}</span>}
              </button>
              {x.translated && (
                <button
                  type="button"
                  title={t("common.remove")}
                  onClick={() => removeTrack(x.language)}
                  className="rounded-full p-0.5 text-faint hover:bg-hover hover:text-fg"
                >
                  <X size={11} />
                </button>
              )}
            </span>
          ))}
        </div>

        {translating ? (
          <div className="flex flex-col gap-1.5">
            <div className="flex items-center gap-2 text-xs text-muted">
              <Loader2 size={13} className="animate-spin text-accent" />
              <span className="flex-1">
                {t("subtitles.translating", { language: t(`languages.${translating.to}`, { defaultValue: translating.to }) })}
              </span>
              <button type="button" className="text-faint hover:text-fg" onClick={cancelTranslation}>
                {t("common.cancel")}
              </button>
            </div>
            <ProgressBar value={Math.max(0.05, translating.progress)} />
          </div>
        ) : aiReady ? (
          <div className="flex flex-col gap-1.5">
            <div className="flex gap-2">
              <Select
                value={target}
                onChange={setTarget}
                options={available.map((l) => ({ value: l, label: t(`languages.${l}`, { defaultValue: l }) }))}
              />
              <Button onClick={() => target && void translate(target)} disabled={!target}>
                <Languages size={14} />
                {t("subtitles.translate")}
              </Button>
            </div>
            <button
              type="button"
              className="flex items-center gap-1 self-start text-xs text-faint hover:text-fg"
              onClick={() => setShowNote(!showNote)}
            >
              {showNote ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
              {t("subtitles.translationNote")}
            </button>
            {showNote && (
              <textarea
                value={sub.translationInstructions}
                onChange={(e) => setSub({ translationInstructions: e.target.value })}
                placeholder={t("subtitles.translationNotePlaceholder")}
                rows={2}
                className="w-full resize-none rounded-md border border-line bg-bg px-2.5 py-2 text-xs text-fg placeholder:text-faint focus:border-accent focus:outline-none"
              />
            )}
          </div>
        ) : (
          <div className="flex items-center gap-2 text-xs text-muted">
            <Languages size={14} className="text-faint" />
            <span className="flex-1">{t("subtitles.needAi")}</span>
            <Button size="sm" onClick={() => openSettings("ai")}>
              {t("empty.configure")}
            </Button>
          </div>
        )}
        {notice && <p className="text-xs text-warn">{notice}</p>}

        <DubSection track={track} />

        <div className="flex flex-col gap-1.5">
          <label className="flex items-center justify-between text-xs text-muted">
            {t("subtitles.preview")}
            <Toggle checked={showCaptions} onChange={setShowCaptions} label={t("subtitles.preview")} />
          </label>
          <div className="flex gap-2">
            <Select
              className="h-7 text-xs"
              value={sub.size}
              onChange={(v) => setSub({ size: v as CaptionSize })}
              options={(["small", "medium", "large"] as const).map((v) => ({ value: v, label: t(`subtitles.size.${v}`) }))}
            />
            <Select
              className="h-7 text-xs"
              value={sub.style}
              onChange={(v) => setSub({ style: v as CaptionStyle })}
              options={(["outline", "box"] as const).map((v) => ({ value: v, label: t(`subtitles.style.${v}`) }))}
            />
          </div>
        </div>

        <button
          type="button"
          className="flex items-center gap-1 self-start text-xs text-faint hover:text-fg"
          onClick={() => setShowOptions(!showOptions)}
        >
          {showOptions ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          {t("subtitles.layout")}
        </button>
        {showOptions && (
          <div className="flex flex-col gap-3 rounded-md border border-line p-3">
            <OptionsEditor value={sub.options} onChange={(o) => setSub({ options: o })} />
            <Button size="sm" onClick={() => void generate()}>
              <RefreshCw size={12} />
              {t("subtitles.regenerate")}
            </Button>
          </div>
        )}
      </div>

      <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto px-1.5 py-1.5">
        {track.cues.map((c) => (
          <CueRow key={`${track.language}-${c.id}`} cue={c} language={track.language} onSeek={() => player.seek(c.start)} />
        ))}
      </div>

      <div className="flex items-center gap-2 border-t border-line p-3">
        <span className="flex-1 truncate text-xs text-faint" title={saved ?? undefined}>
          {saved ? t("subtitles.saved", { file: saved.split(/[\\/]/).pop() }) : t("subtitles.count", { count: track.cues.length })}
        </span>
        <Button size="sm" onClick={() => void saveFile("srt")}>
          <Download size={13} />
          SRT
        </Button>
        <Button size="sm" onClick={() => void saveFile("vtt")}>
          <Download size={13} />
          VTT
        </Button>
      </div>
    </div>
  );
}
