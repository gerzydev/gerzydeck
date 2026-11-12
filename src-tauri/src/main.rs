#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gerzys_deck::commands::{self, EngineTx};
use gerzys_deck::config::Config;
use gerzys_deck::device::{spawn_device, DeckOptions, HidApiBackend};
use gerzys_deck::engine::{spawn_engine, EngineDeps, EventSink, Msg, PluginTile, State};
use gerzys_deck::keyboard::SendInputKeyboard;
use gerzys_deck::os;
use gerzys_deck::render::render_key_jpeg;
use gerzys_deck::plugin_commands::{self, HostTx, PluginNet};
use gerzys_deck::plugins::host::{spawn_host, HostDeps};
use gerzys_deck::plugins::process::Supervisor;
use gerzys_deck::plugins::webview::{CompositeLauncher, HtmlLauncher};
use gerzys_deck::store::{data_dir, Store};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

const TRAY_ID: &str = "main";

struct TauriSink(AppHandle);

impl EventSink for TauriSink {
    fn state(&self, s: &State) {
        let _ = self.0.emit("deck://state", s);
        if let Some(tray) = self.0.tray_by_id(TRAY_ID) {
            let status = if s.connected { "Připojeno" } else { "Nepřipojeno" };
            let tip: String = format!("Gerzy's deck — {status} · {}", s.active_profile_name).chars().take(63).collect();
            // set_tooltip waits for the main thread; never make the engine thread wait for it.
            std::thread::spawn(move || {
                let _ = tray.set_tooltip(Some(tip));
            });
        }
    }
    fn config(&self, c: &Config) {
        let _ = self.0.emit("deck://config", c);
    }
    fn key(&self, key: u8, pressed: bool) {
        let _ = self.0.emit("deck://key", serde_json::json!({ "key": key, "pressed": pressed }));
    }
    fn raw(&self, device_id: u8, pressed: bool) {
        let _ = self.0.emit("deck://raw", serde_json::json!({ "deviceId": device_id, "pressed": pressed }));
    }
}

/// Thread handles joined on quit so the deck goes to sleep and held keys are released.
struct Workers(Mutex<Vec<JoinHandle<()>>>);

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn quit(app: &AppHandle) {
    if let Some(tx) = app.try_state::<EngineTx>() {
        let _ = tx.0.send(Msg::Shutdown);
    }
    // Join the engine/device (releases held keys, puts the deck to sleep) off the main thread,
    // and never wait longer than 3 s — "Ukončit" must always quit.
    if let Some(workers) = app.try_state::<Workers>() {
        let handles: Vec<JoinHandle<()>> = workers.0.lock().unwrap().drain(..).collect();
        let (done_tx, done_rx) = channel();
        std::thread::spawn(move || {
            for h in handles {
                let _ = h.join();
            }
            let _ = done_tx.send(());
        });
        if done_rx.recv_timeout(Duration::from_secs(3)).is_err() {
            log::warn!("shutdown timed out, exiting anyway");
        }
    }
    app.exit(0);
}

fn init_logging(dir: &std::path::Path) {
    use simplelog::{CombinedLogger, ConfigBuilder, LevelFilter, SharedLogger, SimpleLogger, WriteLogger};
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join("log.txt");
    if std::fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("log.old.txt"));
    }
    let cfg = ConfigBuilder::new().set_time_format_rfc3339().build();
    let mut loggers: Vec<Box<dyn SharedLogger>> = vec![SimpleLogger::new(LevelFilter::Info, cfg.clone())];
    if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        loggers.push(WriteLogger::new(LevelFilter::Info, cfg, f));
    }
    let _ = CombinedLogger::init(loggers);
}

/// Plugin-driven key: plugin image + title, plus ✓ / ⚠ feedback and a missing-plugin marker.
fn render_plugin_tile(t: &PluginTile) -> Vec<u8> {
    use gerzys_deck::engine::Overlay;
    use gerzys_deck::plugins::image::{compose_tile, Align, TitleParams};
    // live value from the plugin at the bottom (≈1.6× the SDK's 9 pt), user label small at the top
    let (title, label) = match (&t.title, &t.label) {
        (Some(v), l) => (Some(v.clone()), l.clone()),
        (None, Some(l)) => (Some(l.clone()), None),
        (None, None) => (None, None),
    };
    let title = if t.missing { Some(format!("⚠ {}", title.unwrap_or_default())) } else { title };
    let bottom = TitleParams { bold: true, size: 14, ..TitleParams::default() };
    let mut canvas = compose_tile(t.image.as_deref(), t.bg, title.as_deref(), &bottom);
    if let Some(label) = label {
        let top = TitleParams { bold: true, size: 9, align: Align::Top, ..TitleParams::default() };
        let base = image::DynamicImage::ImageRgb8(canvas.clone()).to_rgba8();
        canvas = compose_tile(Some(&base), t.bg, Some(&label), &top);
    }
    let mark = match t.overlay {
        Overlay::Ok => Some(("✓", [0x43, 0xc4, 0x6a])),
        Overlay::Alert => Some(("⚠", [0xf2, 0xb8, 0x4b])),
        Overlay::None => None,
    };
    if let Some((glyph, color)) = mark {
        let dimmed = image::DynamicImage::ImageRgb8(canvas.clone()).brighten(-60).to_rgba8();
        let big = TitleParams { show: true, size: 30, color, align: Align::Middle, bold: true };
        canvas = compose_tile(Some(&dimmed), t.bg, Some(glyph), &big);
    }
    gerzys_deck::render::tile_jpeg(&canvas)
}

fn start_core(app: &AppHandle) -> anyhow::Result<()> {
    let dir = data_dir();
    let store = Store::new(dir.clone());

    // plugin host: its events go to the engine once the engine exists
    let engine_slot: Arc<Mutex<Option<std::sync::mpsc::Sender<Msg>>>> = Arc::default();
    let slot = engine_slot.clone();
    let host_tx = spawn_host(HostDeps {
        plugins_dir: gerzys_deck::plugins::plugins_dir(),
        settings_file: dir.join("plugin-settings.json"),
        events: Box::new(move |e| {
            if let Some(tx) = slot.lock().unwrap().as_ref() {
                let _ = tx.send(Msg::Plugin(e));
            }
        }),
        launcher: Box::new(CompositeLauncher { html: HtmlLauncher::new(app.clone()), procs: Box::new(Supervisor::new(dir.join("logs/plugins"))) }),
        decode_image: gerzys_deck::plugins::image::decode_plugin_image,
        open_url: |url| os::launch_target(url, &[]),
        list_processes: os::list_process_names,
    });

    let (dev_events_tx, dev_events_rx) = channel();
    let (device_tx, device_handle) = spawn_device(HidApiBackend::default(), DeckOptions::default(), dev_events_tx);
    let (engine_tx, engine_handle) = spawn_engine(EngineDeps {
        store,
        device_tx,
        keyboard: Arc::new(SendInputKeyboard),
        launch: Arc::new(|t: &str, a: &[String]| os::launch_target(t, a)),
        system: Arc::new(|c: &str| os::run_system(c)),
        sink: Box::new(TauriSink(app.clone())),
        render: render_key_jpeg,
        render_plugin: render_plugin_tile,
        host_tx: Some(host_tx.clone()),
    });
    *engine_slot.lock().unwrap() = Some(engine_tx.clone());

    let tx = engine_tx.clone();
    std::thread::Builder::new().name("device-events".into()).spawn(move || {
        for ev in dev_events_rx {
            if tx.send(Msg::Device(ev)).is_err() {
                break;
            }
        }
    })?;

    let (fg_tx, fg_rx) = channel();
    os::spawn_foreground_watcher(Duration::from_millis(500), fg_tx);
    let tx = engine_tx.clone();
    std::thread::Builder::new().name("foreground-events".into()).spawn(move || {
        for fg in fg_rx {
            if tx.send(Msg::Foreground(fg)).is_err() {
                break;
            }
        }
    })?;

    let tx = engine_tx.clone();
    std::thread::Builder::new().name("conflicts".into()).spawn(move || loop {
        if tx.send(Msg::Conflicts(os::find_conflicts(&os::list_process_names()))).is_err() {
            break;
        }
        std::thread::sleep(Duration::from_secs(10));
    })?;

    app.manage(EngineTx(engine_tx));
    app.manage(HostTx(Mutex::new(host_tx)));
    app.manage(PluginNet::new());
    app.manage(Workers(Mutex::new(vec![engine_handle, device_handle])));
    Ok(())
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Otevřít editor", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Ukončit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit_item])?;
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Gerzy's deck")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quit" => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn main() {
    init_logging(&data_dir());
    let hidden = std::env::args().any(|a| a == "--hidden");
    log::info!("Gerzy's deck starting (hidden={hidden})");

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .register_uri_scheme_protocol(gerzys_deck::plugins::scheme::SCHEME, |_ctx, request| {
            let served = gerzys_deck::plugins::scheme::serve(&gerzys_deck::plugins::plugins_dir(), request.uri().path());
            tauri::http::Response::builder()
                .status(served.status)
                .header("Content-Type", served.mime)
                .header("Cache-Control", "no-store")
                .body(served.body)
                .unwrap_or_default()
        })
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .setup(move |app| {
            let handle = app.handle().clone();
            start_core(&handle).map_err(|e| Box::new(std::io::Error::other(e.to_string())))?;
            build_tray(&handle)?;
            if !hidden {
                show_main(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                // a calibration must never outlive the editor window (deck keys would stay dead)
                if let Some(tx) = window.try_state::<EngineTx>() {
                    let _ = tx.0.send(Msg::SetCalibrating(false));
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::get_state,
            commands::set_config,
            commands::set_active_profile,
            commands::set_calibrating,
            commands::import_image,
            commands::image_data_url,
            commands::list_processes,
            commands::list_scancodes,
            commands::get_autostart,
            commands::set_autostart,
            plugin_commands::plugins_list,
            plugin_commands::plugins_catalog,
            plugin_commands::plugins_install,
            plugin_commands::plugins_install_file,
            plugin_commands::plugins_uninstall,
            plugin_commands::pi_open,
            plugin_commands::pi_close,
            plugin_commands::open_plugin_log,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
