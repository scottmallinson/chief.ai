//! Chief — a privacy-first, on-device AI chief of staff.
//!
//! Everything this application does happens on the user's machine: inference
//! runs against a local Ollama instance, and all persistence is local SQLite.
//! No component of this crate may talk to a remote service on its own.

mod agent;
mod db;
mod ollama;
mod work_log;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_sql::Builder::new()
                .add_migrations(db::DB_URL, db::migrations())
                .build(),
        )
        .setup(|app| {
            // One pooled client for the lifetime of the app, pinned to loopback.
            tauri::Manager::manage(app, ollama::Client::new()?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            agent::ask_agent,
            work_log::list_work_logs,
            work_log::create_work_log
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
