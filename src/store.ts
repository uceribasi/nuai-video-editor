import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { convertFileSrc } from "@tauri-apps/api/core";
import i18n, { languageName, resolveLanguage } from "./i18n";
import { api, errorText, isCancelled } from "./lib/api";
import { cutsSignature, manualCut } from "./lib/cuts";
import { renderCaptionPng, stripHeight } from "./lib/captions";
import type {
  AnalyzeOptions,
  AppStatus,
  Cut,
  CutKind,
  ExportResult,
  MediaInfo,
  Project,
  ProgressEvent,
  AudioPlan,
  Settings,
  SubtitlePlan,
  TaskName,
  VoiceStatus,
} from "./lib/types";

export interface Stage {
  id: string;
  progress: number;
  done: boolean;
}

export interface Busy {
  task: Exclude<TaskName, "download">;
  stages: Stage[];
}

export type SettingsTab = "general" | "ai" | "transcription" | "export" | "voice";
export type RightTab = "cuts" | "subtitles";

export type AudioMode = "original" | "replace" | "add";

interface ExportState {
  open: boolean;
  /** Soundtrack choice when dubs exist; `audioLanguage` picks the dub to use. */
  audioMode: AudioMode;
  audioLanguage: string | null;
  output: string;
  running: boolean;
  /** "captions" while burn-in images are drawn, then "render". */
  stage: string;
  burnLanguage: string | null;
  progress: number;
  result: ExportResult | null;
  error: string | null;
}

interface State {
  settings: Settings | null;
  status: AppStatus | null;
  media: MediaInfo | null;
  previewUrl: string | null;
  waveform: number[];
  waveformRate: number;
  project: Project | null;
  busy: Busy | null;
  error: string | null;
  downloads: Record<string, number>;
  selectedCutId: string | null;
  previewEdits: boolean;
  past: Cut[][];
  future: Cut[][];
  settingsOpen: boolean;
  settingsTab: SettingsTab;
  exportState: ExportState;
  rightTab: RightTab;
  subtitleLang: string | null;
  showCaptions: boolean;
  translating: { to: string; progress: number } | null;
  subtitleNotice: string | null;
  /** Latest voice-engine progress: install stages, or a try-out's steps. */
  voiceProgress: { stage: string; progress: number } | null;
  voice: VoiceStatus | null;
  /** A dub being generated: separate → speak → mix. */
  dubbing: { language: string; stage: string; progress: number } | null;
  /** What the preview plays: "original" or the language of a dub. */
  audioSource: string;

  init(): Promise<void>;
  refreshStatus(): Promise<void>;
  updateSettings(patch: (s: Settings) => Settings): void;
  updateOptions(patch: Partial<AnalyzeOptions>): void;
  openFile(path: string): Promise<void>;
  analyze(): Promise<void>;
  cancel(): void;
  setCuts(update: (cuts: Cut[]) => Cut[]): void;
  toggleCut(id: string, enabled?: boolean): void;
  setKindEnabled(kind: CutKind, enabled: boolean): void;
  addManualCut(start: number, end: number): void;
  keepRange(start: number, end: number): void;
  removeCut(id: string): void;
  undo(): void;
  redo(): void;
  selectCut(id: string | null): void;
  setPreviewEdits(on: boolean): void;
  openSettings(tab?: SettingsTab): void;
  closeSettings(): void;
  openExport(): Promise<void>;
  closeExport(): void;
  setExportOutput(path: string): void;
  runExport(): Promise<void>;
  downloadModel(id: string): Promise<void>;
  dismissError(): void;
  setRightTab(tab: RightTab): void;
  generateSubtitles(): Promise<void>;
  translateSubtitles(to: string): Promise<void>;
  cancelTranslation(): void;
  updateCue(language: string, id: number, text: string): void;
  removeTrack(language: string): void;
  selectSubtitleTrack(language: string): void;
  setShowCaptions(on: boolean): void;
  setBurnLanguage(language: string): void;
  refreshVoice(): Promise<void>;
  createDub(language: string): Promise<void>;
  cancelDub(): void;
  setAudioSource(source: string): void;
  setVoiceConsent(on: boolean): void;
  setExportAudio(mode: AudioMode, language?: string | null): void;
}

/** True when subtitles exist but were built for a different set of cuts. */
export function subtitlesStale(project: Project | null, duration: number): boolean {
  const subs = project?.subtitles;
  return !!subs?.tracks.length && subs.cutsSignature !== cutsSignature(project!.cuts, duration);
}

const ANALYZE_STAGES = (o: AnalyzeOptions, hasAudio: boolean): string[] => {
  const stages: string[] = [];
  if (hasAudio) stages.push("audio");
  const wantsSpeech = o.removeRetakes || o.removeFillers || o.removeStutters || o.useAi;
  if (hasAudio && o.transcribe && wantsSpeech) stages.push("transcribe");
  if (o.removeStatic) stages.push("visual");
  if (o.useAi && o.transcribe) stages.push("ai");
  return stages;
};

let settingsTimer: ReturnType<typeof setTimeout> | undefined;
let projectTimer: ReturnType<typeof setTimeout> | undefined;
const HISTORY_LIMIT = 100;

export const useStore = create<State>((set, get) => {
  const persistSettings = (s: Settings) => {
    clearTimeout(settingsTimer);
    settingsTimer = setTimeout(() => void api.saveSettings(s).catch(() => {}), 400);
  };

  const persistProject = () => {
    clearTimeout(projectTimer);
    projectTimer = setTimeout(() => {
      const p = get().project;
      if (p) void api.saveProject(p).catch(() => {});
    }, 600);
  };

  const onProgress = (e: ProgressEvent) => {
    if (e.task === "dub") {
      set((s) => (s.dubbing ? { dubbing: { ...s.dubbing, stage: e.stage, progress: e.progress } } : {}));
      return;
    }
    if (e.task === "voice") {
      set({ voiceProgress: { stage: e.stage, progress: e.progress } });
      return;
    }
    if (e.task === "translate") {
      set((s) => (s.translating ? { translating: { ...s.translating, progress: e.progress } } : {}));
      return;
    }
    if (e.task === "download") {
      set((s) => ({ downloads: { ...s.downloads, [e.stage]: e.progress } }));
      return;
    }
    if (e.task === "export") {
      set((s) => ({ exportState: { ...s.exportState, progress: e.progress } }));
    }
    const busy = get().busy;
    if (!busy || busy.task !== e.task) return;
    const idx = busy.stages.findIndex((st) => st.id === e.stage);
    const stages =
      idx === -1 ? [...busy.stages, { id: e.stage, progress: e.progress, done: false }] : [...busy.stages];
    const current = idx === -1 ? stages.length - 1 : idx;
    stages.forEach((st, i) => {
      if (i < current) stages[i] = { ...st, done: true, progress: 1 };
    });
    stages[current] = { ...stages[current], progress: e.progress };
    set({ busy: { ...busy, stages } });
  };

  return {
    settings: null,
    status: null,
    media: null,
    previewUrl: null,
    waveform: [],
    waveformRate: 50,
    project: null,
    busy: null,
    error: null,
    downloads: {},
    selectedCutId: null,
    previewEdits: true,
    past: [],
    future: [],
    settingsOpen: false,
    settingsTab: "general",
    exportState: {
      open: false,
      audioMode: "original",
      audioLanguage: null,
      output: "",
      running: false,
      stage: "render",
      burnLanguage: null,
      progress: 0,
      result: null,
      error: null,
    },
    rightTab: "cuts",
    subtitleLang: null,
    showCaptions: true,
    translating: null,
    subtitleNotice: null,
    voiceProgress: null,
    voice: null,
    dubbing: null,
    audioSource: "original",

    async init() {
      await listen<ProgressEvent>("progress", (e) => onProgress(e.payload));
      const [settings, status] = await Promise.all([api.getSettings(), api.getStatus()]);
      void i18n.changeLanguage(resolveLanguage(settings.uiLanguage));
      set({ settings, status });
      void get().refreshVoice();
    },

    async refreshStatus() {
      set({ status: await api.getStatus() });
    },

    updateSettings(patch) {
      const current = get().settings;
      if (!current) return;
      const next = patch(current);
      if (next.uiLanguage !== current.uiLanguage) void i18n.changeLanguage(resolveLanguage(next.uiLanguage));
      set({ settings: next });
      persistSettings(next);
    },

    updateOptions(patch) {
      get().updateSettings((s) => ({ ...s, analysis: { ...s.analysis, ...patch } }));
    },

    async openFile(path) {
      set({
        busy: { task: "open", stages: [] },
        error: null,
        selectedCutId: null,
        past: [],
        future: [],
      });
      try {
        const r = await api.openMedia(path);
        set({
          media: r.media,
          previewUrl: convertFileSrc(r.previewPath),
          waveform: r.waveform,
          waveformRate: r.waveformRate,
          project: r.project,
          // A dubbed project opens with its dub playing (and exporting).
          audioSource: Object.keys(r.project?.dubs ?? {})[0] ?? "original",
          subtitleLang: r.project?.subtitles?.tracks[0]?.language ?? null,
          busy: null,
        });
      } catch (e) {
        set({ busy: null, error: isCancelled(e) ? null : errorText(e) });
      }
    },

    async analyze() {
      const { media, settings } = get();
      if (!media || !settings) return;
      const opts = settings.analysis;
      set({
        busy: {
          task: "analyze",
          stages: ANALYZE_STAGES(opts, media.hasAudio).map((id) => ({ id, progress: 0, done: false })),
        },
        error: null,
        selectedCutId: null,
      });
      try {
        const lang = languageName(resolveLanguage(settings.uiLanguage));
        const project = await api.analyze(media.path, opts, lang);
        set({ project, busy: null, past: [], future: [], subtitleLang: null, subtitleNotice: null });
        if (settings.mode === "auto") {
          const ex = settings.export;
          if ((ex.subtitleMode !== "none" || ex.subtitleSidecar) && project.transcript) {
            await get().generateSubtitles();
          }
          await get().openExport();
          await get().runExport();
        }
      } catch (e) {
        set({ busy: null, error: isCancelled(e) ? null : errorText(e) });
      }
    },

    cancel() {
      void api.cancelTask();
    },

    setCuts(update) {
      const project = get().project;
      if (!project) return;
      const cuts = update(project.cuts);
      set((s) => ({
        project: { ...project, cuts },
        past: [...s.past, project.cuts].slice(-HISTORY_LIMIT),
        future: [],
      }));
      persistProject();
    },

    toggleCut(id, enabled) {
      get().setCuts((cuts) => cuts.map((c) => (c.id === id ? { ...c, enabled: enabled ?? !c.enabled } : c)));
    },

    setKindEnabled(kind, enabled) {
      get().setCuts((cuts) => cuts.map((c) => (c.kind === kind ? { ...c, enabled } : c)));
    },

    addManualCut(start, end) {
      if (end - start < 0.05) return;
      const cut = manualCut(start, end);
      get().setCuts((cuts) => [...cuts, cut].sort((a, b) => a.start - b.start));
      set({ selectedCutId: cut.id });
    },

    keepRange(start, end) {
      get().setCuts((cuts) =>
        cuts
          .filter((c) => !(c.kind === "manual" && c.start >= start - 0.01 && c.end <= end + 0.01))
          .map((c) => (c.enabled && c.start < end && c.end > start ? { ...c, enabled: false } : c)),
      );
    },

    removeCut(id) {
      get().setCuts((cuts) => cuts.filter((c) => c.id !== id));
      if (get().selectedCutId === id) set({ selectedCutId: null });
    },

    undo() {
      const { past, project } = get();
      if (!past.length || !project) return;
      const prev = past[past.length - 1];
      set((s) => ({
        project: { ...project, cuts: prev },
        past: s.past.slice(0, -1),
        future: [project.cuts, ...s.future],
      }));
      persistProject();
    },

    redo() {
      const { future, project } = get();
      if (!future.length || !project) return;
      const next = future[0];
      set((s) => ({
        project: { ...project, cuts: next },
        past: [...s.past, project.cuts],
        future: s.future.slice(1),
      }));
      persistProject();
    },

    selectCut(id) {
      set({ selectedCutId: id });
    },

    setPreviewEdits(on) {
      set({ previewEdits: on });
    },

    openSettings(tab) {
      set({ settingsOpen: true, settingsTab: tab ?? get().settingsTab });
    },

    closeSettings() {
      set({ settingsOpen: false });
      void get().refreshStatus();
      void get().refreshVoice();
    },

    async openExport() {
      const media = get().media;
      if (!media || !get().project) return;
      const output = await api.defaultOutputPath(media.path);
      // Export what the player plays: a dub in the player means the dub replaces the
      // original audio. An explicit "add both" choice is kept.
      const dubs = get().project?.dubs ?? {};
      const source = get().audioSource;
      const playingDub = source !== "original" && dubs[source] ? source : null;
      set((s) => ({
        exportState: {
          open: true,
          audioMode:
            s.exportState.audioMode === "add" && Object.keys(dubs).length > 0 ? "add" : playingDub ? "replace" : "original",
          audioLanguage: playingDub ?? s.exportState.audioLanguage,
          output,
          running: false,
          stage: "render",
          burnLanguage: s.exportState.burnLanguage,
          progress: 0,
          result: null,
          error: null,
        },
      }));
    },

    closeExport() {
      if (get().exportState.running) return;
      set((s) => ({ exportState: { ...s.exportState, open: false } }));
    },

    setExportOutput(path) {
      set((s) => ({ exportState: { ...s.exportState, output: path } }));
    },

    async runExport() {
      const { media, settings, exportState } = get();
      let project = get().project;
      if (!media || !project || !settings) return;
      set({
        exportState: { ...exportState, running: true, stage: "render", progress: 0, result: null, error: null },
        busy: { task: "export", stages: [{ id: "render", progress: 0, done: false }] },
      });
      try {
        const ex = settings.export;
        if ((ex.subtitleMode !== "none" || ex.subtitleSidecar) && project.transcript && !project.subtitles?.tracks.length) {
          await get().generateSubtitles();
          project = get().project!;
        }
        const tracks = project.subtitles?.tracks ?? [];
        const plan: SubtitlePlan = {
          embed: ex.subtitleMode === "embed" ? tracks : [],
          sidecar: ex.subtitleSidecar ? tracks : [],
          burn: [],
          burnBlank: "",
        };
        const burnTrack = tracks.find((t) => t.language === exportState.burnLanguage) ?? tracks[0];
        if (ex.subtitleMode === "burn" && burnTrack && media.hasVideo) {
          // Draw each caption the way the preview shows it; the backend overlays them.
          set((s) => ({ exportState: { ...s.exportState, stage: "captions" } }));
          const look = { size: settings.subtitles.size, style: settings.subtitles.style };
          const lines = Math.max(1, ...burnTrack.cues.map((c) => c.text.split("\n").length));
          const strip = stripHeight(media.width, media.height, look, lines);
          plan.burnBlank = renderCaptionPng("", media.width, media.height, look, strip);
          for (const [i, c] of burnTrack.cues.entries()) {
            plan.burn.push({ start: c.start, end: c.end, png: renderCaptionPng(c.text, media.width, media.height, look, strip) });
            if (i % 20 === 0) {
              set((s) => ({ exportState: { ...s.exportState, progress: i / burnTrack.cues.length } }));
              await new Promise((r) => setTimeout(r, 0));
            }
          }
          set((s) => ({ exportState: { ...s.exportState, stage: "render", progress: 0 } }));
        }
        const dubs = project.dubs ?? {};
        const dubLang = exportState.audioLanguage && dubs[exportState.audioLanguage] ? exportState.audioLanguage : Object.keys(dubs)[0];
        let audio: AudioPlan | null = null;
        if (exportState.audioMode === "replace" && dubLang) {
          audio = { replace: dubs[dubLang], replaceLanguage: dubLang, extra: [] };
        } else if (exportState.audioMode === "add" && dubLang) {
          audio = { replace: null, replaceLanguage: null, extra: Object.entries(dubs).map(([language, path]) => ({ path, language })) };
        }
        const result = await api.exportVideo(media.path, project.cuts, exportState.output, plan, audio);
        set((s) => ({ exportState: { ...s.exportState, running: false, result, progress: 1 }, busy: null }));
      } catch (e) {
        const error = isCancelled(e) ? null : errorText(e);
        set((s) => ({ exportState: { ...s.exportState, running: false, error }, busy: null }));
      }
    },

    async downloadModel(id) {
      set((s) => ({ downloads: { ...s.downloads, [id]: 0 } }));
      try {
        await api.downloadModel(id);
      } catch (e) {
        if (!isCancelled(e)) set({ error: errorText(e) });
      } finally {
        set((s) => {
          const downloads = { ...s.downloads };
          delete downloads[id];
          return { downloads };
        });
        await get().refreshStatus();
      }
    },

    dismissError() {
      set({ error: null });
    },

    setRightTab(tab) {
      set({ rightTab: tab });
    },

    async generateSubtitles() {
      const { project, media, settings } = get();
      if (!project?.transcript || !media || !settings) return;
      try {
        const cues = await api.buildSubtitles(project.transcript, project.cuts, media.duration, settings.subtitles.options);
        const language = project.transcript.language;
        const current = get().project ?? project;
        set({
          project: {
            ...current,
            subtitles: { cutsSignature: cutsSignature(current.cuts, media.duration), tracks: [{ language, translated: false, cues }] },
          },
          subtitleLang: language,
          subtitleNotice: null,
        });
        persistProject();
      } catch (e) {
        set({ error: errorText(e) });
      }
    },

    async translateSubtitles(to) {
      const { project, settings } = get();
      const original = project?.subtitles?.tracks.find((t) => !t.translated);
      if (!original || !settings || get().translating) return;
      set({ translating: { to, progress: 0 }, subtitleNotice: null });
      try {
        const r = await api.translateSubtitles(
          original.cues,
          original.language,
          to,
          settings.subtitles.translationInstructions,
          settings.subtitles.options,
        );
        const current = get().project;
        if (!current?.subtitles) return;
        const tracks = [
          ...current.subtitles.tracks.filter((t) => t.language !== to),
          { language: to, translated: true, cues: r.cues },
        ];
        set({
          project: { ...current, subtitles: { ...current.subtitles, tracks } },
          subtitleLang: to,
          subtitleNotice: r.missing > 0 ? i18n.t("subtitles.missing", { count: r.missing }) : null,
        });
        persistProject();
      } catch (e) {
        if (!isCancelled(e)) set({ error: errorText(e) });
      } finally {
        set({ translating: null });
      }
    },

    cancelTranslation() {
      void api.cancelTranslation();
    },

    updateCue(language, id, text) {
      const project = get().project;
      if (!project?.subtitles) return;
      const tracks = project.subtitles.tracks.map((t) =>
        t.language === language ? { ...t, cues: t.cues.map((c) => (c.id === id ? { ...c, text } : c)) } : t,
      );
      set({ project: { ...project, subtitles: { ...project.subtitles, tracks } } });
      persistProject();
    },

    removeTrack(language) {
      const project = get().project;
      if (!project?.subtitles) return;
      const tracks = project.subtitles.tracks.filter((t) => t.language !== language);
      set({
        project: { ...project, subtitles: tracks.length ? { ...project.subtitles, tracks } : null },
        subtitleLang: tracks[0]?.language ?? null,
      });
      persistProject();
    },

    selectSubtitleTrack(language) {
      set({ subtitleLang: language });
    },

    setShowCaptions(on) {
      set({ showCaptions: on });
    },

    async refreshVoice() {
      try {
        set({ voice: await api.voiceStatus() });
      } catch {
        /* the engine is optional */
      }
    },

    async createDub(language) {
      const { project, media } = get();
      const track = project?.subtitles?.tracks.find((t) => t.language === language);
      if (!project?.transcript || !media || !track || get().dubbing) return;
      set({ dubbing: { language, stage: "separate", progress: 0 } });
      try {
        const path = await api.dubCreate(media.path, project.transcript, track);
        const current = get().project;
        if (!current) return;
        set({ project: { ...current, dubs: { ...(current.dubs ?? {}), [language]: path } }, audioSource: language });
        persistProject();
      } catch (e) {
        if (!isCancelled(e)) set({ error: errorText(e) });
      } finally {
        set({ dubbing: null });
      }
    },

    cancelDub() {
      void api.voiceCancel();
    },

    setAudioSource(source) {
      set({ audioSource: source });
    },

    setVoiceConsent(on) {
      const project = get().project;
      if (!project) return;
      set({ project: { ...project, voiceConsent: on } });
      persistProject();
    },

    setExportAudio(mode, language) {
      set((s) => ({ exportState: { ...s.exportState, audioMode: mode, audioLanguage: language ?? s.exportState.audioLanguage } }));
    },

    setBurnLanguage(language) {
      set((s) => ({ exportState: { ...s.exportState, burnLanguage: language } }));
    },
  };
});
