use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use adw::Application;
use async_channel::{Receiver, Sender};
use gtk::gio;
use gtk::glib;
use gstreamer as gst;
use libadwaita as adw;
use gtk4 as gtk;
use tokio::runtime::Runtime;

mod config;
mod portal;
mod recorder;
mod ui;

use config::SharedSettings;
use ui::events::{RecorderEvent, UiCommand};
use ui::preferences::PreferencesWindow;
use ui::style;
use ui::window::AppWindow;

const APP_ID: &str = "dev.local.ScreenRecord";

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    gst::init().expect("failed to initialize GStreamer");
    tracing::info!(version = gst::version_string().as_str(), "gstreamer initialized");

    let runtime = Runtime::new().expect("failed to create tokio runtime");

    let settings: SharedSettings = config::shared(config::load());

    let (cmd_tx, cmd_rx) = async_channel::unbounded::<UiCommand>();
    let (evt_tx, evt_rx) = async_channel::unbounded::<RecorderEvent>();

    runtime.spawn(recorder::run(cmd_rx, evt_tx.clone(), settings.clone()));

    let app = Application::builder().application_id(APP_ID).build();

    app.connect_startup(|_| {
        style::load_css();
    });

    let evt_rx_cell: Rc<RefCell<Option<Receiver<RecorderEvent>>>> =
        Rc::new(RefCell::new(Some(evt_rx)));
    let cmd_tx_for_window = cmd_tx.clone();
    let evt_tx_for_window = evt_tx.clone();

    let settings_for_window = settings.clone();
    app.connect_activate(move |app| {
        register_app_actions(app, &cmd_tx);

        let window = AppWindow::new(
            app,
            cmd_tx_for_window.clone(),
            evt_tx_for_window.clone(),
            settings_for_window.clone(),
        );
        wire_window_actions(app, &window, settings_for_window.clone());

        if let Some(rx) = evt_rx_cell.borrow_mut().take() {
            window.spawn_event_loop(rx);
        }

        window.present();
    });

    let exit_code = app.run();
    drop(runtime);
    exit_code
}

fn register_app_actions(app: &Application, cmd_tx: &Sender<UiCommand>) {
    let act_quit = gio::SimpleAction::new("quit", None);
    let app_weak = app.downgrade();
    let tx = cmd_tx.clone();
    act_quit.connect_activate(move |_, _| {
        let _ = tx.send_blocking(UiCommand::Quit);
        if let Some(app) = app_weak.upgrade() {
            app.quit();
        }
    });
    app.add_action(&act_quit);
    app.set_accels_for_action("app.quit", &["<Primary>q"]);

}

fn wire_window_actions(app: &Application, window: &Rc<AppWindow>, settings: SharedSettings) {
    let act_about = gio::SimpleAction::new("about", None);
    let parent = window.window().clone();
    act_about.connect_activate(move |_, _| {
        let about = gtk::AboutDialog::builder()
            .program_name("Screen Record")
            .logo_icon_name("dev.local.ScreenRecord")
            .version(env!("CARGO_PKG_VERSION"))
            .comments("Запись экрана со звуком на Linux")
            .license_type(gtk::License::Gpl30)
            .transient_for(&parent)
            .modal(true)
            .build();
        about.present();
    });
    app.add_action(&act_about);

    let act_prefs = gio::SimpleAction::new("preferences", None);
    let parent_window = window.window().clone();
    act_prefs.connect_activate(move |_, _| {
        PreferencesWindow::present(&parent_window, settings.clone());
    });
    app.add_action(&act_prefs);
}
