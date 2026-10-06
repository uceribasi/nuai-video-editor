# Contributing to nuai

Thanks for helping. Bug reports, ideas and pull requests are all welcome.

## Getting started

You need Rust (stable), Node.js 22.12+, pnpm, cmake and ffmpeg.

```bash
pnpm install
pnpm tauri dev
```

Most UI work doesn't need the Rust backend: `pnpm dev` serves the interface at
http://localhost:1420 against recorded sample data (`src/dev/mock.ts`). The README's
Development section shows how to create that sample.

## Before you open a pull request

```bash
pnpm typecheck
cd src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```

CI runs the same checks on macOS.

- Keep changes focused; one topic per pull request.
- New UI text goes into both `src/locales/en.ts` and `src/locales/tr.ts`. If you don't
  speak Turkish, add the English text to both and mention it in the pull request.
- The language model never cuts video or invents timestamps; it only picks transcript
  phrases. Please keep it that way, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
- Don't commit videos, audio or model files.

## Reporting bugs

Include your macOS version, your Mac, what you did and what happened. For voice cloning
or dubbing problems, attach
`~/Library/Application Support/app.nuai.editor/voice/worker.log`.

Voice cloning is for voices you own or have permission to use. Please don't open issues
asking to work around the consent step.
