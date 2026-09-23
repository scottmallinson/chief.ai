//! Chief — a privacy-first, on-device AI chief of staff.
//!
//! Everything this application does happens on the user's machine: inference
//! runs in a llama.cpp server Chief ships and supervises itself, and all
//! persistence is local SQLite. No component of this crate may talk to a remote
//! service on its own.

mod adapter;
mod agent;
mod atlassian;
mod background;
mod calendar;
mod clock;
mod connect;
mod context;
mod corpus;
mod daemon;
mod db;
mod engine;
mod github;
mod ical;
mod ingest;
mod integrations;
mod intent;
mod journal;
mod linear;
mod login;
mod microsoft;
// OAuth machinery shared by every provider. Part of the crate's library API,
// the same as `llama` below.
pub mod oauth;
mod perms;
mod probe;
mod profile;
mod propose;
mod proposed;
mod recipe;
mod retrieval;
mod session;
mod settings;
mod setup;
mod sync_state;
mod tools;
mod watcher;
mod weights;
mod work_log;

// The llama.cpp client is a self-contained piece of this crate's library API:
// it models the whole chat contract, including the tool-calling payload the
// orchestrator depends on.
pub mod llama;

use tauri::{Manager, RunEvent, WindowEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // First, so a second launch hands over before it starts anything of
        // its own. With the window hidden in the tray, launching Chief again
        // is the obvious way to get it back, and a second process would be a
        // second daemon and a second engine.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            background::show(app);
        }))
        .plugin(login::plugin())
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
            // And Outlook, for the mailbox and calendar they connected.
            app.manage(microsoft::Client::new()?);
            app.manage(calendar::Client::new()?);
            app.manage(linear::Client::new()?);
            app.manage(atlassian::Client::new()?);
            app.manage(atlassian::rest::Rest::new()?);
            app.manage(connect::Pending::default());
            app.manage(agent::Attention::default());
            // One ingestion pass at a time, whether the daemon asked or a
            // person clicked Refresh.
            app.manage(daemon::Passes::default());

            // Start the model server while the window is still opening, so the
            // first question does not wait for the weights to come off disk.
            engine::start(&app.handle().clone());

            // And give its memory back when nothing is using it. A resident
            // model holds a couple of gigabytes, which on the machines Chief
            // is written for is most of the room there is.
            engine::supervise(&app.handle().clone());

            // Nobody else on this machine needs to be able to read the
            // database the tokens are in. Before anything else touches it.
            perms::prepare(&app.handle().clone());

            // The folder of markdown the user can edit themselves.
            corpus::prepare(&app.handle().clone());

            // Keeps the work log up to date in the background.
            daemon::spawn(&app.handle().clone());
            watcher::spawn(&app.handle().clone());

            // And keeps running after the window closes, so the above has a
            // process to run in.
            background::install(&app.handle().clone());
            // An AppImage that was updated leaves the login entry naming the
            // old file; point it at this one.
            login::refresh(&app.handle().clone());

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                background::close_requested(window, api);
            }
        })
        .invoke_handler(tauri::generate_handler![
            agent::ask_agent,
            background::close_window,
            background::set_keep_running,
            background::window_behaviour,
            login::launch_at_login,
            login::set_launch_at_login,
            connect::add_calendar,
            connect::add_linear_key,
            connect::add_atlassian_token,
            daemon::sync_now,
            connect::start_login,
            connect::finish_login,
            connect::sign_in_registrations,
            connect::set_sign_in_registration,
            connect::clear_sign_in_registration,
            connect::account_data,
            connect::connections,
            connect::disconnect,
            connect::label_account,
            sync_state::sync_status,
            corpus::corpus_location,
            corpus::list_corpus,
            corpus::read_corpus_file,
            corpus::write_corpus_file,
            corpus::set_corpus_root,
            probe::run_doctor,
            propose::refine_draft,
            proposed::dismiss_proposal,
            proposed::list_proposals,
            proposed::save_proposal,
            profile::bootstrap_profile,
            profile::profile_plan,
            recipe::generate_brief,
            recipe::todays_brief,
            setup::check_readiness,
            setup::download_model,
            setup::start_engine,
            work_log::list_work_logs,
            work_log::create_work_log
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| match event {
            // The engine is our child process. Nothing else will stop it, and a
            // model left resident would hold a couple of gigabytes after the
            // window has gone.
            RunEvent::Exit => app.state::<engine::Engine>().stop(),
            // Clicking the Dock icon of an app whose window is hidden.
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => background::show(app),
            _ => {}
        });
}
