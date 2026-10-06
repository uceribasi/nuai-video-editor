//! Headless nuai: analyze a video and optionally export the edited version.
//!
//!   cargo run --release --example cli -- input.mp4 [--model path/to/ggml.bin] [--lang tr]
//!                                        [--ai] [--provider codex|anthropic|openai|compatible]
//!                                        [--static] [--export out.mp4] [--dump out.json]
//!                                        [--subtitles] [--translate en,de] [--srt out.srt]
//!   cargo run --example cli -- - --provider codex --list-models
//!   cargo run --example cli -- - --voice-install
//!   cargo run --example cli -- - --voice-try sample.mp3 "Text to speak" en
//!
//! `--subtitles` builds captions from the transcript, `--translate` adds AI translations,
//! `--srt` writes one `out.<lang>.srt` per track and `--export` embeds them.
//!
//! `--dump` writes the media info, waveform and project as JSON (the browser UI mocks use it).
//!
//! Uses the same settings file as the app for AI provider and export preferences.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Result};
use nuai_lib::{
    ai, audio, dub, edit, media, pipeline, render, settings, subtitles, task, tools, voice,
};

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn config_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/app.nuai.editor")
    } else {
        home.join(".config/app.nuai.editor")
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(input) = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .map(PathBuf::from)
    else {
        bail!("usage: cli <input> [--model ggml.bin] [--lang tr] [--ai] [--static] [--export out.mp4]");
    };

    let mut s = settings::load(&config_dir().join("settings.json"));
    if let Some(m) = arg_value(&args, "--model") {
        s.transcription.custom_model_path = m;
    }
    if let Some(l) = arg_value(&args, "--lang") {
        s.transcription.language = l;
    }
    if let Some(m) = arg_value(&args, "--ai-model") {
        s.ai.codex_model = m.clone();
        s.ai.anthropic_model = m.clone();
        s.ai.openai_model = m.clone();
        s.ai.compatible_model = m;
    }
    if let Some(p) = arg_value(&args, "--provider") {
        s.ai.provider = serde_json::from_value(serde_json::json!(if p == "codex" {
            "codexCli".to_string()
        } else {
            p
        }))?;
    }
    let voice_paths = voice::VoicePaths::new(&config_dir());
    if args.iter().any(|a| a == "--voice-install") {
        eprintln!("{:?}", voice::status(&voice_paths));
        let started = std::time::Instant::now();
        let last = std::sync::Mutex::new(String::new());
        let progress = |stage: &str, p: f64| {
            let line = if p < 0.0 {
                format!("{stage}…")
            } else {
                format!("{stage} {:3.0}%", p * 100.0)
            };
            let mut l = last.lock().unwrap();
            if *l != line {
                eprintln!("{line}");
                *l = line;
            }
        };
        voice::install(&voice_paths, &progress, &task::Cancel::default()).await?;
        eprintln!(
            "installed in {:.1}s: {:?}",
            started.elapsed().as_secs_f64(),
            voice::status(&voice_paths)
        );
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--voice-try") {
        let (reference, text, lang) = (&args[i + 1], &args[i + 2], &args[i + 3]);
        let ctx = pipeline::AnalyzeContext {
            tools: tools::locate(Some(&s.ffmpeg_path))?,
            settings: s.clone(),
            models_dir: config_dir().join("models"),
            cache_dir: std::env::temp_dir().join("nuai-cli-cache"),
            whisper: Arc::new(tokio::sync::Mutex::new(None)),
            reason_language: "English".into(),
            stage: Arc::new(|_: &str| Arc::new(|_| {})),
            cancel: task::Cancel::default(),
        };
        let started = std::time::Instant::now();
        let reference = dub::build_reference(
            &ctx,
            std::path::Path::new(reference),
            &ctx.cache_dir.join("voice"),
        )
        .await?;
        eprintln!(
            "reference ({:.1}s): {}",
            started.elapsed().as_secs_f64(),
            reference.text
        );
        let out = std::env::temp_dir().join("nuai-voice-try.wav");
        let line = dub::Line {
            text: text.clone(),
            language: Some(lang.clone()),
            duration: None,
            out: out.clone(),
        };
        let mut worker = None;
        let t = std::time::Instant::now();
        dub::speak(
            &voice_paths,
            &mut worker,
            &reference,
            &[line],
            &(Arc::new(|_| {}) as task::Progress),
            &task::Cancel::default(),
        )
        .await?;
        eprintln!(
            "spoke in {:.1}s (incl. model load) → {}",
            t.elapsed().as_secs_f64(),
            out.display()
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--list-models") {
        let client = ai::AiClient::from_settings(&s.ai)?;
        for m in client.list_models().await? {
            println!(
                "{:<28} {:<24} {}",
                m.id,
                m.efforts.join(","),
                m.description.unwrap_or_default()
            );
        }
        return Ok(());
    }
    let mut opts = s.analysis.clone();
    opts.use_ai = args.iter().any(|a| a == "--ai");
    opts.remove_static = args.iter().any(|a| a == "--static");

    let tools = tools::locate(Some(&s.ffmpeg_path))?;
    let cache = std::env::temp_dir().join("nuai-cli-cache");
    let ctx = pipeline::AnalyzeContext {
        tools: tools.clone(),
        settings: s.clone(),
        models_dir: config_dir().join("models"),
        cache_dir: cache.clone(),
        whisper: Arc::new(tokio::sync::Mutex::new(None)),
        reason_language: "English".into(),
        stage: Arc::new(|stage: &str| {
            eprintln!("» {stage}");
            Arc::new(|_| {})
        }),
        cancel: task::Cancel::default(),
    };

    let started = std::time::Instant::now();
    let project = pipeline::analyze(&ctx, &input, &opts).await?;
    eprintln!("analysis took {:.1}s", started.elapsed().as_secs_f64());

    if let Some(t) = &project.transcript {
        println!("\nTranscript ({}, {}):", t.language, t.model);
        for u in &t.utterances {
            println!("  {:>4} [{:6.2}-{:6.2}] {}", u.id, u.start, u.end, u.text);
        }
    }
    println!("\nCuts (threshold {:.1} dB):", project.threshold_db);
    for c in &project.cuts {
        println!(
            "  {} {:<12} {:6.2}-{:6.2} conf {:.2}  {}",
            if c.enabled { "✔" } else { "·" },
            c.id,
            c.start,
            c.end,
            c.confidence,
            c.reason
        );
    }
    for w in &project.warnings {
        println!("  ! {}: {}", w.code, w.message);
    }
    for n in &project.notes {
        println!("  AI: {n}");
    }
    let edited = edit::edited_duration(&project.cuts, project.duration);
    println!(
        "\n{:.1}s → {:.1}s ({:.0}% shorter)",
        project.duration,
        edited,
        100.0 * (1.0 - edited / project.duration)
    );

    if let Some(dump) = arg_value(&args, "--dump") {
        let info = media::probe(&tools, &input).await?;
        let pcm = media::extract_pcm(
            &tools,
            &input,
            info.duration,
            &(Arc::new(|_| {}) as task::Progress),
            &task::Cancel::default(),
        )
        .await?;
        let json = serde_json::json!({
            "media": info,
            "waveform": audio::waveform(&audio::envelope(&pcm)),
            "waveformRate": audio::WAVEFORM_RATE,
            "project": project,
        });
        std::fs::write(&dump, serde_json::to_vec(&json)?)?;
        eprintln!("wrote {dump}");
    }

    let translate = arg_value(&args, "--translate");
    let mut tracks: Vec<subtitles::SubtitleTrack> = Vec::new();
    if args.iter().any(|a| a == "--subtitles")
        || translate.is_some()
        || arg_value(&args, "--srt").is_some()
    {
        let Some(t) = &project.transcript else {
            bail!("subtitles need a transcript")
        };
        let opts = &s.subtitles.options;
        let cues = subtitles::build_cues(
            &t.utterances,
            &project.cuts,
            project.duration,
            &t.language,
            opts,
        );
        tracks.push(subtitles::SubtitleTrack {
            language: t.language.clone(),
            translated: false,
            cues,
        });
        for lang in translate
            .iter()
            .flat_map(|l| l.split(','))
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            let client = ai::AiClient::from_settings(&s.ai)?;
            eprintln!("translating to {lang} with {}", client.model_label());
            let started = std::time::Instant::now();
            let tr = ai::translate::translate(
                &client,
                &tracks[0].cues,
                &t.language,
                lang,
                &s.subtitles.translation_instructions,
                &(Arc::new(|_| {}) as task::Progress),
                &task::Cancel::default(),
            )
            .await?;
            eprintln!(
                "translated to {lang} in {:.1}s ({} missing)",
                started.elapsed().as_secs_f64(),
                tr.missing
            );
            tracks.push(subtitles::SubtitleTrack {
                language: lang.to_string(),
                translated: true,
                cues: subtitles::rewrap(&tr.cues, opts, lang),
            });
        }
        for track in &tracks {
            println!(
                "\nSubtitles [{}] ({} cues):",
                track.language,
                track.cues.len()
            );
            for c in &track.cues {
                println!(
                    "  {:6.2}-{:6.2}  {}",
                    c.start,
                    c.end,
                    c.text.replace('\n', " / ")
                );
            }
        }
    }
    if let Some(srt) = arg_value(&args, "--srt") {
        let info = media::probe(&tools, &input).await?;
        let keeps = render::export_keeps(&info, &project.cuts);
        for track in &tracks {
            let path = render::sidecar_path(&PathBuf::from(&srt), &track.language);
            render::write_subtitle_file(track, &keeps, &path)?;
            eprintln!("wrote {}", path.display());
        }
    }

    if let Some(out) = arg_value(&args, "--export") {
        let info = media::probe(&tools, &input).await?;
        let encoders = media::available_encoders(&tools).await?;
        let plan = render::SubtitlePlan {
            embed: tracks.clone(),
            ..Default::default()
        };
        // --dub <lang>: speak that subtitle track with the video's own voice and use it as the audio.
        let mut audio_plan = render::AudioPlan::default();
        if let Some(lang) = arg_value(&args, "--dub") {
            let track = tracks.iter().find(|t| t.language == lang).ok_or_else(|| {
                anyhow::anyhow!("no subtitle track {lang}; add --translate {lang}")
            })?;
            let ctx = pipeline::AnalyzeContext {
                tools: tools.clone(),
                settings: s.clone(),
                models_dir: config_dir().join("models"),
                cache_dir: cache.clone(),
                whisper: Arc::new(tokio::sync::Mutex::new(None)),
                reason_language: "English".into(),
                stage: Arc::new(|stage: &str| {
                    eprintln!("» dub: {stage}");
                    Arc::new(|_| {})
                }),
                cancel: task::Cancel::default(),
            };
            let media_dir = pipeline::media_cache_dir(&cache, &input)?;
            let mut worker = None;
            let started = std::time::Instant::now();
            let t = project.transcript.as_ref().unwrap();
            let mv =
                dub::prepare_media_voice(&ctx, &voice_paths, &mut worker, &input, t, &media_dir)
                    .await?;
            eprintln!("reference: {}", mv.reference.text);
            let dubbed = dub::dub_track(
                &ctx,
                &voice_paths,
                &mut worker,
                &mv,
                track,
                project.duration,
                &media_dir,
            )
            .await?;
            eprintln!(
                "dubbed in {:.1}s → {}",
                started.elapsed().as_secs_f64(),
                dubbed.display()
            );
            audio_plan.replace = Some(dubbed.to_string_lossy().into_owned());
            audio_plan.replace_language = Some(lang);
        }
        let progress: task::Progress = Arc::new(|p| eprint!("\rexporting {:3.0}%", p * 100.0));
        let started = std::time::Instant::now();
        let r = render::export(
            &tools,
            &encoders,
            &info,
            &project.cuts,
            &PathBuf::from(&out),
            &s.export,
            &plan,
            &audio_plan,
            &cache.join("render"),
            &progress,
            &task::Cancel::default(),
        )
        .await?;
        eprintln!(
            "\nexported {} segments → {} ({:.1}s) in {:.1}s",
            r.segments,
            r.output,
            r.duration,
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}
