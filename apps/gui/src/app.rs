//! The Tauri shell: commands the page may call and the events it listens to. The page is
//! untrusted input: every argument is validated here, paths and item references are resolved on
//! this side, and the Nebius key never reaches the page.
mod action_commands;
mod elevation_commands;
mod history_commands;
mod image_commands;
mod measurements;
mod preview_commands;
mod selection_commands;
mod settings_commands;
mod state;
mod storage_commands;
mod turn_commands;
mod worker;

use crate::images::ImageSlot;
use crate::model_routing::ModelRouting;
use crate::selection::SelectionRegistry;
use crate::{elevation, settings, shell, view};
use history_commands::{open_history, open_sampler};
use measurements::{battery_thread, measure_thread};
use pcagent_agent::capabilities::Probes;
use pcagent_storage::StorageService;
use pcagent_telemetry::ProgramNames;
use state::{AppState, ScanSlot};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Manager};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn run() {
    if pcagent_inspect::elevation::ensure_unelevated_host().is_err() {
        eprintln!("Start PC Agent from a standard, unelevated session.");
        pcagent_inspect::elevation::notify_elevated_host();
        std::process::exit(2);
    }
    let background = std::env::args_os().any(|arg| arg == "--background");
    let mut context = tauri::generate_context!();
    if background {
        for window in &mut context.config_mut().app.windows {
            window.visible = false;
        }
    }
    let (tx, rx) = mpsc::channel();
    let selections = Arc::new(Mutex::new(SelectionRegistry::default()));
    let measured_selections = selections.clone();
    // One pending request at most: a read still running absorbs further asks.
    let (battery_poke, battery_pokes) = mpsc::sync_channel(1);
    let battery = Arc::new(Mutex::new(view::LiveBattery::default()));
    let (battery_latest, battery_selections) = (battery.clone(), selections.clone());
    // One file check, no launch: the helper is only ever started from the app's own folder.
    let probes = Arc::new(Mutex::new(Probes {
        elevated_helper: pcagent_inspect::elevation::helper_installed(),
        ..Probes::default()
    }));
    let (battery_probes, measured_probes) = (probes.clone(), probes.clone());
    let program_names = Arc::new(ProgramNames::new());
    let measured_names = program_names.clone();
    thread::Builder::new()
        .name("battery".into())
        .spawn(move || {
            battery_thread(
                battery_pokes,
                battery_latest,
                battery_selections,
                battery_probes,
            )
        })
        .expect("the battery thread starts");
    thread::Builder::new()
        .name("measurements".into())
        .spawn(move || {
            measure_thread(
                rx,
                measured_selections,
                battery,
                battery_poke,
                measured_probes,
                measured_names,
            )
        })
        .expect("the measurement thread starts");
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            measure: Mutex::new(tx),
            storage: Arc::new(StorageService::new()),
            scan: Arc::new(Mutex::new(ScanSlot::default())),
            investigation: Mutex::new(None),
            next_id: AtomicU64::new(1),
            history: Arc::new(Mutex::new(open_history())),
            sampler: open_sampler(),
            privacy: Mutex::new(settings::load()),
            actions: Arc::new(Mutex::new(None)),
            elevation: Arc::new(elevation::Slot::default()),
            preview: Mutex::new(None),
            images: Mutex::new(ImageSlot::default()),
            image_decoding: Arc::new(AtomicBool::new(false)),
            preview_revision: AtomicU64::new(0),
            selection_context_revision: AtomicU64::new(0),
            selections,
            model_routing: Mutex::new(ModelRouting::load()),
            probes,
            program_names,
        })
        .invoke_handler(tauri::generate_handler![
            settings_commands::app_info,
            turn_commands::current_turn,
            measurements::dashboard,
            measurements::process_activity,
            measurements::local_browser_windows,
            action_commands::action_plans,
            action_commands::startup_restore_prepare,
            action_commands::browser_manager_prepare,
            action_commands::action_decline,
            action_commands::action_run,
            preview_commands::cloud_preview,
            image_commands::image_prepare,
            image_commands::image_discard,
            image_commands::conversation_image_get,
            selection_commands::selection_sources,
            selection_commands::selection_preview,
            selection_commands::selection_send,
            settings_commands::model_routing_get,
            settings_commands::model_routing_set,
            turn_commands::start_investigation,
            turn_commands::answer_extension,
            turn_commands::stop_investigation,
            storage_commands::quick_folders,
            storage_commands::pick_folder,
            storage_commands::start_scan,
            storage_commands::cancel_scan,
            storage_commands::scan_page,
            storage_commands::reveal,
            storage_commands::scan_children,
            storage_commands::scan_item_facts,
            settings_commands::save_cloud_settings,
            settings_commands::save_search_key,
            settings_commands::open_web_source,
            settings_commands::test_cloud,
            settings_commands::cloud_models,
            settings_commands::open_nebius_console,
            elevation_commands::elevation_prepare,
            elevation_commands::elevation_status,
            elevation_commands::elevation_run,
            elevation_commands::elevation_cancel,
            history_commands::conversations_list,
            history_commands::conversation_get,
            history_commands::conversation_delete,
            history_commands::conversations_clear,
            history_commands::history_status,
            history_commands::metric_history,
            history_commands::record_activity_start,
            history_commands::battery_recording,
            history_commands::record_activity_cancel,
            history_commands::history_clear,
            settings_commands::startup_get,
            settings_commands::startup_set,
            settings_commands::privacy_get,
            settings_commands::privacy_set,
            turn_commands::send_message,
            turn_commands::stop_turn
        ])
        .setup(|app| {
            use tauri::{
                menu::{Menu, MenuItem},
                tray::{TrayIconBuilder, TrayIconEvent},
            };
            // Drawn by Windows, outside the page: in the language of Windows itself.
            let texts = shell::shell_texts(pcagent_inspect::elevation::windows_ui_is_french());
            let open = MenuItem::with_id(app, "open", texts.tray_open, true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", texts.tray_quit, true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let mut tray = TrayIconBuilder::with_id("pcagent")
                .tooltip(texts.tray_tooltip)
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        shutdown(app);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                        if let Some(window) = tray.app_handle().get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&format!("PC Agent {}", env!("CARGO_PKG_VERSION")));
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if pcagent_inspect::autostart::get().unwrap_or(false) {
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    // Tauri exits the process without Rust destructors. Wait for the helper first.
                    api.prevent_close();
                    shutdown(window.app_handle());
                }
            }
        })
        .run(context)
        .expect("PC Agent's window could not start (is Microsoft Edge WebView2 installed?)");
}

fn shutdown(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Some(shutdown) = state.elevation.begin_shutdown() else {
        return;
    };
    if let Some(running) = lock(&state.investigation).as_ref() {
        running.stop.store(true, Ordering::SeqCst);
        if let Some(answer) = lock(&running.answer).take() {
            let _ = answer.send(false);
        }
    }
    if let Some(sampler) = &state.sampler {
        sampler.stop();
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if shutdown.wait(Duration::from_secs(5)) {
            app.exit(0);
        } else {
            eprintln!("PC Agent is waiting for its elevated inspection to stop; try Quit again.");
        }
    });
}
