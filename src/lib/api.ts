import { invoke } from "@tauri-apps/api/core";
import type {
  AiSettings,
  AudioPlan,
  AnalyzeOptions,
  AppStatus,
  Cue,
  Cut,
  ExportResult,
  KeyProvider,
  ModelInfo,
  OpenResult,
  Project,
  Settings,
  SubtitleOptions,
  SubtitlePlan,
  SubtitleTrack,
  Transcript,
  VoiceStatus,
} from "./types";

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  getStatus: () => invoke<AppStatus>("get_status"),
  setApiKey: (provider: KeyProvider, key: string) =>
    invoke<void>("set_api_key", { provider, key }),
  listAiModels: (ai: AiSettings) => invoke<ModelInfo[]>("list_ai_models", { ai }),
  testAi: (ai: AiSettings) => invoke<string>("test_ai", { ai }),
  downloadModel: (id: string) => invoke<void>("download_model", { id }),
  cancelDownload: () => invoke<void>("cancel_download"),
  deleteModel: (id: string) => invoke<void>("delete_model", { id }),
  cancelTask: () => invoke<void>("cancel_task"),
  openMedia: (path: string) => invoke<OpenResult>("open_media", { path }),
  analyze: (path: string, options: AnalyzeOptions, reasonLanguage: string) =>
    invoke<Project>("analyze", { path, options, reasonLanguage }),
  saveProject: (project: Project) => invoke<void>("save_project", { project }),
  defaultOutputPath: (path: string) => invoke<string>("default_output_path", { path }),
  exportVideo: (path: string, cuts: Cut[], output: string, subtitles: SubtitlePlan | null, audio: AudioPlan | null) =>
    invoke<ExportResult>("export_video", { path, cuts, output, subtitles, audio }),
  dubCreate: (path: string, transcript: Transcript, track: SubtitleTrack) =>
    invoke<string>("dub_create", { path, transcript, track }),
  buildSubtitles: (transcript: Transcript, cuts: Cut[], duration: number, options: SubtitleOptions) =>
    invoke<Cue[]>("build_subtitles", { transcript, cuts, duration, options }),
  rewrapSubtitles: (cues: Cue[], options: SubtitleOptions, language: string) =>
    invoke<Cue[]>("rewrap_subtitles", { cues, options, language }),
  translateSubtitles: (cues: Cue[], from: string, to: string, instructions: string, options: SubtitleOptions) =>
    invoke<{ cues: Cue[]; missing: number }>("translate_subtitles", { cues, from, to, instructions, options }),
  cancelTranslation: () => invoke<void>("cancel_translation"),
  exportSubtitles: (path: string, cuts: Cut[], track: SubtitleTrack, output: string) =>
    invoke<void>("export_subtitles", { path, cuts, track, output }),
  defaultSubtitlePath: (path: string, language: string) =>
    invoke<string>("default_subtitle_path", { path, language }),
  voiceStatus: () => invoke<VoiceStatus>("voice_status"),
  voiceInstall: () => invoke<VoiceStatus>("voice_install"),
  voiceCancel: () => invoke<void>("voice_cancel"),
  voiceUninstall: () => invoke<VoiceStatus>("voice_uninstall"),
  voiceTry: (reference: string, text: string, language: string) =>
    invoke<string>("voice_try", { reference, text, language }),
};

/** Tauri rejects with the Rust error string; normalize anything else. */
export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

export const isCancelled = (e: unknown) => errorText(e).includes("Cancelled");
