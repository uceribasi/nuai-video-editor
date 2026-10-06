//! nuai core. The modules are public so the headless CLI (`examples/cli.rs`) can drive them.

pub mod ai;
pub mod audio;
mod commands;
pub mod dub;
pub mod edit;
pub mod media;
pub mod models;
pub mod pipeline;
pub mod render;
pub mod settings;
pub mod speech;
pub mod subtitles;
pub mod task;
pub mod tools;
pub mod transcribe;
pub mod vad;
pub mod voice;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // ggml registers Metal buffers in "residency sets" and aborts at process exit if any
    // are still alive. The whisper model stays loaded until the app quits (possibly in
    // the middle of an analysis), so quitting crashed. Transcription speed is unchanged
    // without them. Set before any thread starts.
    if std::env::var_os("GGML_METAL_NO_RESIDENCY").is_none() {
        std::env::set_var("GGML_METAL_NO_RESIDENCY", "1");
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let paths = app.path();
            let config = paths.app_config_dir()?;
            let cache = paths.app_cache_dir()?;
            let models = paths.app_data_dir()?.join("models");
            app.manage(commands::AppState::new(
                config.join("settings.json"),
                cache,
                models,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::get_status,
            commands::set_api_key,
            commands::list_ai_models,
            commands::test_ai,
            commands::download_model,
            commands::cancel_download,
            commands::delete_model,
            commands::cancel_task,
            commands::open_media,
            commands::analyze,
            commands::save_project,
            commands::default_output_path,
            commands::export_video,
            commands::build_subtitles,
            commands::rewrap_subtitles,
            commands::translate_subtitles,
            commands::cancel_translation,
            commands::export_subtitles,
            commands::default_subtitle_path,
            commands::voice_status,
            commands::voice_install,
            commands::voice_cancel,
            commands::voice_uninstall,
            commands::voice_try,
            commands::dub_create,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
