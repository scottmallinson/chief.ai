//! Chief — a privacy-first, on-device AI chief of staff.
//!
//! Everything this application does happens on the user's machine: inference
//! runs against a local Ollama instance, and all persistence is local SQLite.
//! No component of this crate may talk to a remote service on its own.

mod agent;
mod clock;
mod connect;
mod daemon;
mod db;
mod github;
mod integrations;
mod session;
mod setup;
mod tools;
mod work_log;

// The Ollama client is a self-contained piece of this crate's library API: it
// models the whole chat contract, including the tool-calling payload the
// orchestrator uses in the next step.
pub mod ollama;

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
            // One pooled client each, for the lifetime of the app. The Ollama
            // client is pinned to loopback; the GitHub one is pinned to GitHub.
            tauri::Manager::manage(app, ollama::Client::new()?);
            tauri::Manager::manage(app, github::Client::new()?);
            tauri::Manager::manage(app, connect::Pending::default());

            // Keeps the work log up to date in the background.
            daemon::spawn(&app.handle().clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            agent::ask_agent,
            connect::start_github_login,
            connect::finish_github_login,
            connect::github_connection,
            connect::disconnect_github,
            setup::check_readiness,
            setup::pull_model,
            work_log::list_work_logs,
            work_log::create_work_log
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
