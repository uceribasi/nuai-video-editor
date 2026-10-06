# Architecture

nuai is a Tauri 2 app: a React UI in the system webview and a Rust core that drives
ffmpeg and whisper.cpp. The core is also usable headless (`src-tauri/examples/cli.rs`).

## Principles

1. **The AI decides, it doesn't edit.** Models receive transcript text with phrase ids
   and answer with JSON that names ids (and optionally exact words inside one phrase).
   Timestamps always come from our own measurements, so a model can't hallucinate a cut
   into the middle of a word.
2. **Works without AI.** Silence, retake, filler, stutter and freeze detection are
   rule-based and run offline. The AI review refines them.
3. **Everything is a reversible cut.** Analysis produces an edit decision list
   (`Project.cuts`). Nothing touches the media until export; the UI toggles cuts and the
   preview skips them live.

## Pipeline (`pipeline.rs`)

```
probe ─► decode audio (16 kHz mono) ─► Silero VAD ─► speech activity (10 ms frames)
            │                                 ├─► silent regions ─► silence cuts
            ▼                                 │
        whisper.cpp ─► words ─► re-time on speech ─► phrases ─► retakes / stutters / fillers
                                                          │
freezedetect ─► frozen spans minus speech ─► static cuts  │
                                                          ▼
                                         AI review (optional) ─► final cut list
```

### Silence

Pauses are found from *speech* activity, not loudness: a quiet sentence ending over a
music bed is louder than the threshold allows but still speech, and music between
sentences is loud but not speech. `vad.rs` runs Silero VAD v5 (via whisper.cpp, the
model is bundled, ~0.9 MB) and maps its per-window speech probability onto the 10 ms
frame grid as a 0 / -100 dB "envelope", so the rest of the pipeline is unchanged. The
UI's sensitivity slider moves the probability cut-off.

If the VAD can't run, `audio.rs` falls back to loudness: RMS per 10 ms frame in dBFS,
with an automatic threshold 26 dB under the speech level (95th percentile) but at least
4 dB above the noise floor (10th percentile). Either way, blips under 60 ms (clicks,
keyboard) don't break a pause, each cut leaves `padding` seconds of air next to speech,
and cut points snap to the quietest nearby frame of the real loudness envelope.

### Transcript and word timing

`transcribe.rs` runs whisper.cpp through whisper-rs (Metal on macOS) with token
timestamps and, when the model is recognized, DTW alignment heads, which give much
better word onsets. Tokens are byte pieces (a Turkish "ş" can span two), so bytes are
collected per word and decoded once.

Whisper still places the first words after a long pause too early. `retime_words` uses
the DTW onsets as anchors and re-times every word on the speech activity: a word
can't start inside a pause (it moves to the next voiced frame), starts where its sound
starts, and ends where the voice stops or the next word begins. Phrases whose span is
almost silent are dropped as hallucinations. Transcripts are cached per file, model and
language.

### Rules (`speech.rs`)

- **Phrases**: words split on pauses of 0.45 s or more and on sentence punctuation.
- **Retakes**: a phrase is an abandoned take when one of the next three phrases (within
  20 s) starts the same way (common prefix of 2 or more words) and repeats at least 60% of
  it (LCS over normalized words, cut-off words match their full form). Short fragments
  that the next phrase begins with are false starts. The cut runs from the abandoned
  phrase to the start of the new attempt.
- **Stutters**: repeated groups of 1–4 words inside a phrase. Single-word repeats start
  disabled because many languages repeat words on purpose ("yavaş yavaş").
- **Fillers**: a multilingual list plus drawn-out single sounds ("eeee", "mmm").
  Whisper gets a language-specific prompt that makes it write hesitations down.

Word-derived cut edges snap to the quietest frame within 120 ms.

### Static picture

ffmpeg `freezedetect` on a downscaled stream finds frozen spans; speech spans (from the
transcript, or from the envelope without one) are subtracted so a still slide with
narration is never cut.

### AI review (`ai/review.rs`)

The model gets the phrases as `u12 [83.4-86.1] text`, the rule-based retake and stutter
findings as hints, optional user instructions and an optional target duration, and
must answer with:

```json
{
  "cuts": [{ "from": "u12", "to": "u13", "phrase": "", "kind": "retake",
             "reason": "…", "confidence": 0.9 }],
  "notes": "…"
}
```

`phrase` non-empty with `from == to` removes those exact words inside one phrase. The
AI's answer replaces the rule-based retakes and stutters; silence, filler and static cuts
stay. Long transcripts are reviewed in chunks of 300 phrases with overlap.

Providers (`ai/`):

| Provider | Transport | Structured output |
|---|---|---|
| Anthropic | Messages API | `output_config.format` JSON schema, effort; server-side refusal fallback on models that support it; plain retry on 400 |
| OpenAI | Responses API | `text.format` strict JSON schema |
| OpenAI-compatible | Chat Completions | `json_schema` → `json_object` → plain, whichever the server accepts |
| Codex CLI | `codex exec` in an empty temp folder, read-only sandbox, user config ignored, newest installed Codex (PATH, Codex.app, ChatGPT.app; the model catalog OpenAI returns depends on the client version), models from `codex debug models`, one retry | `--output-schema` |

Answers are parsed leniently (code fences and `<think>` blocks are stripped).

## Subtitles (`subtitles.rs`, `ai/translate.rs`)

Cues are built from the transcript words whose midpoint survives the cuts. A cue ends
when it would exceed `maxChars × maxLines`, run longer than `maxDuration`, at pauses of
0.8 s or more, after a sentence once it has some text, and at phrase or clause
boundaries when already well filled. Text is wrapped into balanced lines (breaking after
punctuation when it helps); scripts without spaces (Chinese, Japanese, Thai…) are joined
without them. Cues stay on screen 0.3 s past the last word and at least `minDuration`
when there is room, without overlapping.

Cues are stored on the original timeline together with a fingerprint of the cuts, so
the UI can say when they are out of date. On export they are mapped onto the edited
timeline with the same frame-aligned kept segments the renderer uses (otherwise
per-segment rounding would make captions drift over long videos).

Translation sends cues as `id: text` lines in chunks of 120, with the previous cues as
context, and the model must return the same ids. Timing never changes; translated text
is re-wrapped for the target language.

Burn-in: the UI draws each caption to a transparent PNG strip with the same function
that draws the live preview (sizes are relative to the video, measured on the short
side). The renderer writes one ffconcat playlist per kept segment (captions and blank
gaps in segment-local time) and composites it with `overlay`, which every ffmpeg build
has, unlike the libass-based `subtitles` filter. Selectable tracks are muxed as
`mov_text` with ISO 639-2 language tags.

## Voice engine (`voice.rs`, `dub.rs`, `resources/voice/`)

Optional and installed on demand into `<app data>/voice`:

1. **Runtime:** an installed `uv` is used, otherwise the official standalone binary is
   downloaded. uv creates a venv on its own managed Python 3.11, never the system one.
   `requirements.txt` + `constraints.txt` pin the full dependency tree; a fingerprint
   of them in `installed.json` triggers a reinstall when they change.
2. **Models:** `manifest.json` pins Hugging Face repos to exact revisions with file
   sizes and SHA-256. Files go into a private HF cache layout
   (`models/hf/hub/models--org--name/snapshots/<rev>`), so OmniVoice and Demucs both
   load offline. Matching files in the user's own HF cache are hard-linked; otherwise
   our downloader fetches them with HTTP range resume, a 60 s stall timeout and retries,
   then verifies the hash.
3. **Worker:** `nuai_voice.py` runs as a long-lived process speaking JSON lines
   (`ping`, `separate`, `clone`) with progress events; stderr goes to `voice/worker.log`.
   It loads weights sequentially (`HF_DEACTIVATE_ASYNC_LOAD`) because PyTorch's MPS
   kernel cache isn't thread-safe and transformers' threaded loading randomly crashes
   or hangs on Apple Silicon.

`dub::build_reference` transcribes any recording and cuts its best 4–10 s of continuous
speech (`voice::pick_reference`) as the cloning reference; `dub::speak` batches lines
through OmniVoice with optional target durations (fp16, 16 diffusion steps: about
real time on an M1 Pro).

### Dubbing

The voice is cloned from the video itself, so the dub sounds like the person on screen:

1. `prepare_media_voice` extracts the audio and runs Demucs (`separate`) into
   `vocals.wav` and `background.wav` (music, noise, room), cached per media file.
2. The reference is the best 5–10 s of continuous speech from the transcript (gaps of at
   most 0.6 s), cut from `vocals.wav` so music doesn't leak into the cloned voice; its
   transcript text goes with it.
3. `dub_track` speaks every cue of a subtitle track with a target duration that fits
   the original sentence and never runs into the next cue (`target_duration`). Clips
   are placed at their cue start in a full-length 24 kHz track, gain-matched to the
   separated vocals (`volumedetect`, ±12 dB), and mixed over the background with
   ffmpeg. The result is cached by a hash of the cues.
4. The dub is kept on the original timeline, so it follows any later cut. On export an
   `AudioPlan` either replaces the source audio or adds dubs as extra, language-tagged
   audio tracks; segments read the same `-ss/-t` window from the dub WAV as from the
   video.

The UI requires a consent checkbox (stored in the project) before cloning, and the
player can preview the dub in sync with the muted video.

## Edit decision list (`edit.rs`)

Every cut has a kind, source (rule / AI / user), confidence and an enabled flag. Cuts
under 0.6 confidence are proposed but start disabled. For playback and export, enabled
cuts are merged, and kept fragments shorter than 120 ms are absorbed. The same logic is
mirrored in `src/lib/cuts.ts` so the UI updates instantly.

## Export (`render.rs`)

Each kept segment is encoded on its own: accurate input seek, constant frame rate, 12 ms
audio fades at both ends, PCM audio. Segments are aligned to whole frames so audio and
video lengths agree, encode three at a time, and are joined with the concat demuxer
(video copied, audio encoded once to AAC). This keeps memory flat for long videos with
hundreds of cuts. Encoders are chosen by availability: VideoToolbox on macOS, then
x264/x265, NVENC, QSV, AMF, OpenH264.

## Preview

The webview plays the source directly when it can (MP4/MOV with H.264, or HEVC on
macOS); otherwise a 720p H.264 proxy with short GOPs is made once and cached. While
"skip cuts" is on, a `requestAnimationFrame` loop jumps over merged cut ranges.

## Storage

- Settings: `<app config>/settings.json`; API keys in the OS keychain.
- Per-file cache `<app cache>/media/<hash of path, size, mtime>/`: transcripts, preview
  proxy and `project.json` (cuts and manual edits, restored when the file is reopened).
- Models: `<app data>/models`.
