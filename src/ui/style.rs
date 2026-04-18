use gtk::gdk;
use gtk4 as gtk;

const STYLE_CSS: &str = include_str!("../../data/style.css");

pub fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(STYLE_CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        // Регистрируем собственный поиск иконок в dev-режиме, чтобы при `cargo run`
        // находилась иконка из `data/icons`. В production (после install.sh) иконки
        // уже в `~/.local/share/icons/hicolor` — дефолтный путь theme.
        let theme = gtk::IconTheme::for_display(&display);
        for path in dev_icon_search_paths() {
            theme.add_search_path(&path);
        }
    } else {
        tracing::warn!("no default GDK display, CSS not applied");
    }
}

fn dev_icon_search_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join("data/icons"));
    }
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        paths.push(std::path::PathBuf::from(manifest).join("data/icons"));
    }
    paths
}
