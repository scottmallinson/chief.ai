//! Chief — a privacy-first, on-device AI chief of staff.
//!
//! Everything this application does happens on the user's machine: inference
//! runs in a llama.cpp server Chief ships and supervises itself, and all
//! persistence is local SQLite. No component of this crate may talk to a remote
//! service on its own.

mod agent;
mod clock;
mod connect;
mod daemon;
mod db;
mod engine;
mod github;
mod integrations;
// OAuth machinery shared by every provider. Part of the crate's library API,
// the same as `llama` below.
pub mod oauth;
mod probe;
mod session;
mod setup;
mod tools;
mod weights;
mod work_log;

// The llama.cpp client is a self-contained piece of this crate's library API:
// it models the whole chat contract, including the tool-calling payload the
// orchestrator depends on.
pub mod llama;

use tauri::{Manager, RunEvent};

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
            // The engine decides where inference happens; the client is pointed
            // at it and refuses to be pointed anywhere off this machine.
            let engine = engine::Engine::discover(&app.handle().clone())?;
            let llama = llama::Client::with_base_url(engine.base_url())?;

            app.manage(engine);
            app.manage(llama);
            // Pinned to GitHub, for the account the user connected.
            app.manage(github::Client::new()?);
            app.manage(connect::Pending::default());
            app.manage(agent::Attention::default());

            // Start the model server while the window is still opening, so the
            // first question does not wait for the weights to come off disk.
            engine::start(&app.handle().clone());

            // And give its memory back when nothing is using it. A resident
            // model holds a couple of gigabytes, which on the machines Chief
            // is written for is most of the room there is.
            engine::supervise(&app.handle().clone());

            // Keeps the work log up to date in the background.
            daemon::spawn(&app.handle().clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            agent::ask_agent,
            connect::start_login,
            connect::finish_login,
            connect::connections,
            connect::disconnect,
            connect::label_account,
            probe::run_doctor,
            setup::check_readiness,
            setup::download_model,
            setup::start_engine,
            work_log::list_work_logs,
            work_log::create_work_log
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // The engine is our child process. Nothing else will stop it, and a
            // model left resident would hold a couple of gigabytes after the
            // window has gone.
            if let RunEvent::Exit = event {
                app.state::<engine::Engine>().stop();
            }
        });
}
