/**
 * Browser-only stand-in for the Rust backend, so the UI can be developed with
 * `pnpm dev` in a normal browser. Never bundled into the app.
 *
 * It replays real analysis data from `public/dev/sample.json` + `sample.mp4`, which
 * the CLI produces:
 *   cargo run --example cli -- public/dev/sample.mp4 --lang tr --static --dump public/dev/sample.json
 */
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { mergedCuts } from "../lib/cuts";
import type { AppStatus, Cue, Cut, Project, Settings, SubtitleOptions, Transcript } from "../lib/types";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** English for the sentences of the sample video, a language tag for everything else. */
const SAMPLE_EN: [string, string][] = [
  ["merhaba", "Hi everyone, today I'll show you our new project."],
  ["gereksiz", "This app automatically removes the unnecessary parts of your videos."],
  ["sessizlik", "For example, it removes long silences and repeated sentences."],
  ["sonuç", "As a result, your video gets much shorter and more engaging."],
];
const fakeTranslation = (text: string, to: string) =>
  (to === "en" && SAMPLE_EN.find(([key]) => text.toLocaleLowerCase("tr").includes(key))?.[1]) || `[${to.toUpperCase()}] ${text}`;

const settings: Settings = {
  uiLanguage: "system",
  mode: "review",
  analysis: {
    removeSilence: true,
    minSilence: 0.5,
    padding: 0.15,
    sensitivity: 0,
    transcribe: true,
    removeRetakes: true,
    removeFillers: true,
    removeStutters: true,
    removeStatic: true,
    minStatic: 2,
    useAi: false,
    instructions: "",
    targetDuration: 0,
  },
  ai: {
    // `?demo` turns AI on so translation works (used for README screenshots).
    provider: new URLSearchParams(location.search).has("demo") ? "codexCli" : "none",
    anthropicModel: "claude-opus-5-5",
    openaiModel: "",
    compatibleBaseUrl: "http://localhost:11434/v1",
    compatibleModel: "",
    codexModel: "",
    codexPath: "",
    effort: "medium",
  },
  transcription: { model: "large-v3-turbo-q5_0", customModelPath: "", language: "auto", useGpu: true },
  export: { codec: "h264", quality: "high", outputDir: "", suffix: "_edited", subtitleMode: "none", subtitleSidecar: false },
  subtitles: {
    options: { maxChars: 42, maxLines: 2, maxDuration: 6, minDuration: 1 },
    size: "medium",
    style: "outline",
    translationInstructions: "",
  },
  ffmpegPath: "",
};

const status: AppStatus = {
  codex: { path: "/Applications/Codex.app/Contents/Resources/codex-cli/bin/codex", version: "0.159.0", loggedIn: true, detail: "Logged in using ChatGPT" },
  ffmpeg: { ffmpeg: "/opt/homebrew/bin/ffmpeg", ffprobe: "/opt/homebrew/bin/ffprobe" },
  ffmpegError: null,
  models: [
    { id: "large-v3-turbo-q5_0", file: "ggml-large-v3-turbo-q5_0.bin", sizeMb: 547, multilingual: true, recommended: true, installed: true },
    { id: "large-v3-turbo", file: "ggml-large-v3-turbo.bin", sizeMb: 1549, multilingual: true, recommended: false, installed: false },
    { id: "small", file: "ggml-small.bin", sizeMb: 466, multilingual: true, recommended: false, installed: false },
    { id: "base", file: "ggml-base.bin", sizeMb: 142, multilingual: true, recommended: false, installed: true },
    { id: "tiny", file: "ggml-tiny.bin", sizeMb: 75, multilingual: true, recommended: false, installed: false },
  ],
  modelsDir: "~/Library/Application Support/app.nuai.editor/models",
  apiKeys: { anthropic: false, openai: false, compatible: false },
};

interface Sample {
  media: { path: string } & Record<string, unknown>;
  waveform: number[];
  waveformRate: number;
  project: Project;
}

let sample: Sample | null = null;
async function loadSample(): Promise<Sample> {
  if (!sample) sample = (await (await fetch("/dev/sample.json")).json()) as Sample;
  return sample;
}

async function fakeProgress(task: string, stage: string, ms: number) {
  const steps = 12;
  for (let i = 0; i <= steps; i++) {
    await emit("progress", { task, stage, progress: i / steps });
    await sleep(ms / steps);
  }
}

/** Rough stand-in for `subtitles::build_cues` so the panel has something to show. */
function buildCues(transcript: Transcript, cuts: Cut[], duration: number, o: SubtitleOptions): Cue[] {
  const removed = mergedCuts(cuts, duration);
  const cues: Cue[] = [];
  for (const u of transcript.utterances) {
    const words = u.words.filter((w) => !removed.some(([s, e]) => (w.start + w.end) / 2 >= s && (w.start + w.end) / 2 < e));
    let group: typeof words = [];
    const flush = () => {
      if (!group.length) return;
      const text = group.map((w) => w.text).join(" ");
      const half = Math.ceil(group.length / 2);
      cues.push({
        id: cues.length + 1,
        start: group[0].start,
        end: group[group.length - 1].end + 0.3,
        text: text.length > o.maxChars && o.maxLines > 1
          ? `${group.slice(0, half).map((w) => w.text).join(" ")}\n${group.slice(half).map((w) => w.text).join(" ")}`
          : text,
      });
      group = [];
    };
    for (const w of words) {
      if (group.map((x) => x.text).join(" ").length + w.text.length > o.maxChars * o.maxLines) flush();
      group.push(w);
    }
    flush();
  }
  return cues;
}

let voiceInstalled = true;
const voiceStatus = () => ({
  installed: voiceInstalled,
  runtimeReady: voiceInstalled,
  modelsReady: voiceInstalled,
  downloadMb: voiceInstalled ? 0 : 4551,
  totalMb: 4551,
  root: "~/Library/Application Support/app.nuai.editor/voice",
});

export function installMocks() {
  mockWindows("main");
  (window as unknown as { __TAURI_INTERNALS__: { convertFileSrc: (p: string) => string } }).__TAURI_INTERNALS__.convertFileSrc =
    (p: string) => p;

  mockIPC(
    async (cmd, args) => {
      const a = (args ?? {}) as Record<string, unknown>;
      switch (cmd) {
        case "get_settings":
          return structuredClone(settings);
        case "save_settings":
          Object.assign(settings, a.settings);
          return null;
        case "get_status":
          return structuredClone(status);
        case "open_media": {
          await fakeProgress("open", "audio", 500);
          const s = await loadSample();
          return {
            media: { ...s.media, path: "/dev/sample.mp4", fileName: "sample.mp4" },
            previewPath: "/dev/sample.mp4",
            waveform: s.waveform,
            waveformRate: s.waveformRate,
            project: null,
          };
        }
        case "analyze": {
          const s = await loadSample();
          for (const stage of ["audio", "transcribe", "visual"]) await fakeProgress("analyze", stage, 600);
          return { ...structuredClone(s.project), mediaPath: "/dev/sample.mp4" };
        }
        case "save_project":
        case "set_api_key":
        case "cancel_task":
        case "cancel_download":
        case "delete_model":
          return null;
        case "default_output_path":
          return "/Users/you/Movies/sample_edited.mp4";
        case "export_video":
          await fakeProgress("export", "render", 1500);
          return { output: "/Users/you/Movies/sample_edited.mp4", duration: 18, segments: 5, subtitleFiles: [] };
        case "download_model":
          await fakeProgress("download", String(a.id), 2000);
          return null;
        case "list_ai_models":
          if ((a.ai as Settings["ai"]).provider === "codexCli") {
            return [
              { id: "gpt-6-astra", name: "GPT-6-Astra", description: "Frontier intelligence for the most demanding work.", efforts: ["low", "medium", "high", "xhigh", "max", "ultra"], defaultEffort: "medium" },
              { id: "gpt-6-luna", name: "GPT-6-Luna", description: "Fast and affordable model for easier tasks.", efforts: ["low", "medium", "high", "xhigh", "max"], defaultEffort: "medium" },
            ];
          }
          return [{ id: "claude-opus-5-5", name: "Claude Opus 5.5" }, { id: "claude-sonnet-5-5", name: "Claude Sonnet 5.5" }];
        case "build_subtitles":
          return buildCues(a.transcript as Transcript, a.cuts as Cut[], a.duration as number, a.options as SubtitleOptions);
        case "rewrap_subtitles":
          return a.cues;
        case "translate_subtitles":
          await fakeProgress("translate", String(a.to), 1500);
          return { cues: (a.cues as Cue[]).map((c) => ({ ...c, text: fakeTranslation(c.text, String(a.to)) })), missing: 0 };
        case "default_subtitle_path":
          return `/Users/you/Movies/sample_edited.${String(a.language)}.srt`;
        case "export_subtitles":
        case "cancel_translation":
        case "voice_cancel":
          return null;
        case "voice_status":
          return voiceStatus();
        case "voice_install":
          for (const stage of ["runtime", "packages"]) {
            await emit("progress", { task: "voice", stage, progress: -1 });
            await sleep(900);
          }
          await fakeProgress("voice", "models", 2000);
          voiceInstalled = true;
          return voiceStatus();
        case "voice_uninstall":
          voiceInstalled = false;
          return voiceStatus();
        case "dub_create":
          for (const stage of ["separate", "speak", "mix"]) await fakeProgress("dub", stage, 900);
          return "/dev/sample.mp4";
        case "voice_try":
          for (const stage of ["audio", "transcribe"]) await fakeProgress("voice", stage, 500);
          await fakeProgress("voice", "speak", 900);
          return "/dev/sample.mp4";
        case "test_ai":
          throw "Mock mode: no backend";
        case "plugin:dialog|open":
          return "/dev/sample.mp4";
        case "plugin:dialog|save":
          return "/Users/you/Movies/sample_edited.tr.srt";
        default:
          console.warn("unmocked command", cmd, args);
          return null;
      }
    },
    { shouldMockEvents: true },
  );
}
