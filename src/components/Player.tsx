import { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { AudioLines, Pause, Play, SkipBack, SkipForward } from "lucide-react";
import { useStore } from "../store";
import { mergedCuts, rangeAt } from "../lib/cuts";
import { drawCaption } from "../lib/captions";
import { formatTime } from "../lib/format";
import { player } from "../lib/player";
import { IconButton, Toggle, cx } from "./ui";

export function Player() {
  const { t } = useTranslation();
  const media = useStore((s) => s.media);
  const previewUrl = useStore((s) => s.previewUrl);
  const project = useStore((s) => s.project);
  const previewEdits = useStore((s) => s.previewEdits);
  const setPreviewEdits = useStore((s) => s.setPreviewEdits);
  const showCaptions = useStore((s) => s.showCaptions);
  const subtitleLang = useStore((s) => s.subtitleLang);
  const captionLook = useStore((s) => s.settings?.subtitles);
  const audioSource = useStore((s) => s.audioSource);
  const setAudioSource = useStore((s) => s.setAudioSource);
  const dubRef = useRef<HTMLAudioElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const captionRef = useRef<HTMLCanvasElement>(null);
  const timeRef = useRef<HTMLSpanElement>(null);
  const editedRef = useRef<HTMLSpanElement>(null);
  const [playing, setPlaying] = useState(false);

  const duration = media?.duration ?? 0;
  const merged = useMemo(() => (project ? mergedCuts(project.cuts, duration) : []), [project, duration]);
  const editedTotal = useMemo(() => duration - merged.reduce((s, [a, b]) => s + (b - a), 0), [merged, duration]);
  const dubs = project?.dubs ?? {};
  const dubPath = audioSource !== "original" ? dubs[audioSource] : undefined;
  const dubUrl = useMemo(() => (dubPath ? convertFileSrc(dubPath) : undefined), [dubPath]);

  // While a dub plays, the video is muted and the dub follows it.
  useEffect(() => {
    const v = videoRef.current;
    if (v) v.muted = !!dubUrl;
    if (!dubUrl) dubRef.current?.pause();
  }, [dubUrl]);

  const captionCues = useMemo(() => {
    const tracks = project?.subtitles?.tracks ?? [];
    return (tracks.find((t) => t.language === subtitleLang) ?? tracks[0])?.cues ?? [];
  }, [project, subtitleLang]);

  useEffect(() => {
    player.attach(videoRef.current);
    return () => player.attach(null);
  }, [previewUrl]);

  // One loop does it all: skip over cuts while playing, update the clocks, draw captions.
  useEffect(() => {
    let raf = 0;
    let lastCaption = "";
    const tick = () => {
      const v = videoRef.current;
      const canvas = captionRef.current;
      if (v && canvas && captionLook) {
        const text = showCaptions ? captionCues.find((c) => v.currentTime >= c.start && v.currentTime < c.end)?.text ?? "" : "";
        const w = v.clientWidth;
        const h = v.clientHeight;
        const key = `${text}|${w}x${h}|${captionLook.size}|${captionLook.style}`;
        if (key !== lastCaption) {
          lastCaption = key;
          const dpr = window.devicePixelRatio || 1;
          canvas.style.left = `${v.offsetLeft}px`;
          canvas.style.top = `${v.offsetTop}px`;
          canvas.style.width = `${w}px`;
          canvas.style.height = `${h}px`;
          canvas.width = Math.round(w * dpr);
          canvas.height = Math.round(h * dpr);
          const ctx = canvas.getContext("2d")!;
          ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
          ctx.clearRect(0, 0, w, h);
          if (text) drawCaption(ctx, text, w, h, captionLook);
        }
      }
      const dub = dubRef.current;
      if (v && dub && dubUrl) {
        if (Math.abs(dub.currentTime - v.currentTime) > 0.12) dub.currentTime = v.currentTime;
        if (!v.paused && dub.paused) void dub.play().catch(() => {});
        if (v.paused && !dub.paused) dub.pause();
      }
      if (v) {
        if (previewEdits && !v.paused) {
          const r = rangeAt(merged, v.currentTime + 0.01);
          if (r) v.currentTime = Math.min(r[1] + 0.001, duration);
        }
        const now = v.currentTime;
        if (timeRef.current) timeRef.current.textContent = formatTime(now, true);
        if (editedRef.current) {
          const removed = merged.reduce((s, [a, b]) => s + Math.max(0, Math.min(b, now) - a), 0);
          editedRef.current.textContent = formatTime(Math.max(0, now - removed));
        }
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [merged, previewEdits, duration, captionCues, showCaptions, captionLook, dubUrl]);

  if (!media || !previewUrl) return null;

  return (
    <div className="flex min-h-0 flex-col">
      <div className="relative flex min-h-0 flex-1 items-center justify-center bg-black">
        <video
          ref={videoRef}
          src={previewUrl}
          className="max-h-full max-w-full"
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
          onClick={() => player.toggle()}
          preload="auto"
          playsInline
        />
        <canvas ref={captionRef} className="pointer-events-none absolute" />
        {dubUrl && <audio ref={dubRef} src={dubUrl} preload="auto" />}
        {!media.hasVideo && (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-faint">
            <AudioLines size={48} />
          </div>
        )}
      </div>
      <div className="flex h-11 shrink-0 items-center gap-1 border-t border-line bg-panel px-2">
        <IconButton label="−5s" onClick={() => player.nudge(-5)}>
          <SkipBack size={15} />
        </IconButton>
        <IconButton label={playing ? "Pause" : "Play"} onClick={() => player.toggle()}>
          {playing ? <Pause size={17} /> : <Play size={17} />}
        </IconButton>
        <IconButton label="+5s" onClick={() => player.nudge(5)}>
          <SkipForward size={15} />
        </IconButton>
        <div className="ml-2 font-mono text-xs">
          <span ref={timeRef} className="text-fg">
            0:00.0
          </span>
          <span className="text-faint"> / {formatTime(duration)}</span>
        </div>
        {project && (
          <div className="ml-4 font-mono text-xs text-muted">
            {t("player.edited")} <span ref={editedRef} className="text-fg">0:00</span>
            <span className="text-faint"> / {formatTime(editedTotal)}</span>
          </div>
        )}
        {Object.keys(dubs).length > 0 && (
          <select
            value={audioSource}
            onChange={(e) => setAudioSource(e.target.value)}
            title={t("player.audio")}
            className="ml-auto h-7 rounded-md border border-line bg-bg px-2 text-xs text-fg focus:border-accent focus:outline-none"
          >
            <option value="original">{t("player.originalAudio")}</option>
            {Object.keys(dubs).map((l) => (
              <option key={l} value={l}>
                {t("player.dubAudio", { language: t(`languages.${l}`, { defaultValue: l }) })}
              </option>
            ))}
          </select>
        )}
        <label
          className={cx("flex items-center gap-2 text-xs text-muted", Object.keys(dubs).length === 0 && "ml-auto")}
          title={t("player.previewEditsHint")}
        >
          {t("player.previewEdits")}
          <Toggle checked={previewEdits} onChange={setPreviewEdits} label={t("player.previewEdits")} />
        </label>
      </div>
    </div>
  );
}
