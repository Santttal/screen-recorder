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
    } else {
        tracing::warn!("no default GDK display, CSS not applied");
    }
}
