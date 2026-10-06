// Mirrors of the Rust types (serde camelCase).

export type CutKind =
  | "silence"
  | "retake"
  | "filler"
  | "stutter"
  | "static"
  | "mistake"
  | "offtopic"
  | "shorten"
  | "manual";

export type CutSource = "rule" | "ai" | "user";

export interface Cut {
  id: string;
  start: number;
  end: number;
  kind: CutKind;
  reason: string;
  code: string;
  detail: string;
  confidence: number;
  enabled: boolean;
  source: CutSource;
}

export interface Word {
  text: string;
  start: number;
  end: number;
  prob: number;
}

export interface Utterance {
  id: string;
  start: number;
  end: number;
  text: string;
  words: Word[];
}

export interface Transcript {
  language: string;
  model: string;
  utterances: Utterance[];
}

export interface Notice {
  code: string;
  message: string;
}

export interface AnalyzeOptions {
  removeSilence: boolean;
  minSilence: number;
  padding: number;
  sensitivity: number;
  transcribe: boolean;
  removeRetakes: boolean;
  removeFillers: boolean;
  removeStutters: boolean;
  removeStatic: boolean;
  minStatic: number;
  useAi: boolean;
  instructions: string;
  targetDuration: number;
}

export interface Cue {
  id: number;
  /** Seconds on the original timeline. */
  start: number;
  end: number;
  /** May contain line breaks. */
  text: string;
}

export interface SubtitleOptions {
  maxChars: number;
  maxLines: number;
  maxDuration: number;
  minDuration: number;
}

export interface SubtitleTrack {
  language: string;
  translated: boolean;
  cues: Cue[];
}

export interface Subtitles {
  cutsSignature: string;
  tracks: SubtitleTrack[];
}

export type CaptionSize = "small" | "medium" | "large";
export type CaptionStyle = "outline" | "box";

export interface SubtitleSettings {
  options: SubtitleOptions;
  size: CaptionSize;
  style: CaptionStyle;
  translationInstructions: string;
}

export interface BurnCue {
  start: number;
  end: number;
  png: string;
}

export interface SubtitlePlan {
  embed: SubtitleTrack[];
  sidecar: SubtitleTrack[];
  burn: BurnCue[];
  burnBlank: string;
}

export interface Project {
  version: number;
  mediaPath: string;
  duration: number;
  cuts: Cut[];
  transcript: Transcript | null;
  thresholdDb: number;
  notes: string[];
  warnings: Notice[];
  options: AnalyzeOptions;
  aiModel: string | null;
  subtitles?: Subtitles | null;
  /** Dubbed audio per language (WAV paths on the original timeline). */
  dubs?: Record<string, string>;
  voiceConsent?: boolean;
}

export interface AudioPlan {
  replace: string | null;
  replaceLanguage: string | null;
  extra: { path: string; language: string }[];
}

export interface MediaInfo {
  path: string;
  fileName: string;
  duration: number;
  size: number;
  hasVideo: boolean;
  hasAudio: boolean;
  width: number;
  height: number;
  fps: number;
  videoCodec: string | null;
  audioCodec: string | null;
  container: string;
  bitRate: number | null;
  videoBitRate: number | null;
  playable: boolean;
}

export interface OpenResult {
  media: MediaInfo;
  previewPath: string;
  waveform: number[];
  waveformRate: number;
  project: Project | null;
}

export type AiProvider = "none" | "anthropic" | "openai" | "compatible" | "codexCli";

/** Providers authenticated with an API key stored in the keychain. */
export type KeyProvider = "anthropic" | "openai" | "compatible";

export interface AiSettings {
  provider: AiProvider;
  anthropicModel: string;
  openaiModel: string;
  compatibleBaseUrl: string;
  compatibleModel: string;
  codexModel: string;
  codexPath: string;
  effort: string;
}

export interface TranscriptionSettings {
  model: string;
  customModelPath: string;
  language: string;
  useGpu: boolean;
}

export interface ExportSettings {
  codec: "h264" | "hevc";
  quality: "high" | "balanced" | "small";
  outputDir: string;
  suffix: string;
  subtitleMode: "none" | "embed" | "burn";
  subtitleSidecar: boolean;
}

export type EditMode = "review" | "auto";

export interface Settings {
  uiLanguage: "system" | "en" | "tr";
  mode: EditMode;
  analysis: AnalyzeOptions;
  ai: AiSettings;
  transcription: TranscriptionSettings;
  export: ExportSettings;
  subtitles: SubtitleSettings;
  ffmpegPath: string;
}

export interface ModelStatus {
  id: string;
  file: string;
  sizeMb: number;
  multilingual: boolean;
  recommended: boolean;
  installed: boolean;
}

export interface CodexStatus {
  path: string | null;
  version: string | null;
  loggedIn: boolean;
  detail: string;
}

export interface AppStatus {
  codex: CodexStatus;
  ffmpeg: { ffmpeg: string; ffprobe: string } | null;
  ffmpegError: string | null;
  models: ModelStatus[];
  modelsDir: string;
  apiKeys: Record<KeyProvider, boolean>;
}

export interface ExportResult {
  output: string;
  duration: number;
  segments: number;
  subtitleFiles: string[];
}

export interface ModelInfo {
  id: string;
  name: string;
  description?: string;
  /** Reasoning effort levels the model accepts; absent when unknown. */
  efforts?: string[];
  defaultEffort?: string;
}

export type TaskName = "open" | "analyze" | "export" | "download" | "translate" | "voice" | "dub";

export interface VoiceStatus {
  installed: boolean;
  runtimeReady: boolean;
  modelsReady: boolean;
  downloadMb: number;
  totalMb: number;
  root: string;
}

export interface ProgressEvent {
  task: TaskName;
  stage: string;
  progress: number;
}
