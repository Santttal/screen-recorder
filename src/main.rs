use adw::prelude::*;
use adw::Application;
use gtk::glib;
use gstreamer as gst;
use libadwaita as adw;
use gtk4 as gtk;

mod config;
mod portal;
mod recorder;
mod ui;

const APP_ID: &str = "dev.local.ScreenRecord";

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    gst::init().expect("failed to initialize GStreamer");
    tracing::info!(version = gst::version_string().as_str(), "gstreamer initialized");

    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(|_app| {
        tracing::info!("application activated (window TBD in phase 2)");
    });

    app.run()
}
