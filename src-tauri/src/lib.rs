//! Tauri shell: forwards UI commands to `urs_core::Core` and relays events.
//! All logic lives in the core crate.

mod commands;

use std::sync::Arc;
use tauri::{Emitter, Manager, RunEvent, WindowEvent};
use urs_core::paths::Paths;
use urs_core::Core;

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let core = Core::new(
                Paths::from_env(),
                Arc::new(move |event| {
                    let _ = handle.emit("urs-event", event);
                }),
            )
            .map_err(|d| d.summary)?;
            app.manage(core);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window ends the app (and every preview window).
            if window.label() == "main" && matches!(event, WindowEvent::Destroyed) {
                window.app_handle().exit(0);
            }
        })
        .invoke_handler(commands::handler())
        .build(tauri::generate_context!())
        .expect("failed to start Universal Remotion Studio");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            // Stop preview servers, installs and renders before exiting.
            if let Some(core) = handle.try_state::<Arc<Core>>() {
                core.shutdown();
            }
        }
    });
}
