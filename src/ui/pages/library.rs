//! Экран «Библиотека». Phase 19.b.4 — file-scanner + grid карточек.
//! Без SQLite: пересканируем `settings.output_dir` при каждом открытии
//! и при явном refresh (после сохранения новой записи).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use libadwaita as adw;
use gtk4 as gtk;

use crate::config::SharedSettings;
use crate::library::{ensure_thumb, scan, Recording};

type OpenCallback = Rc<RefCell<Option<Box<dyn Fn(PathBuf)>>>>;

pub struct LibraryPage {
    pub root: gtk::Widget,
    flow: gtk::FlowBox,
    search: gtk::SearchEntry,
    meta_label: gtk::Label,
    settings: SharedSettings,
    on_open: OpenCallback,
    items: Rc<RefCell<Vec<Recording>>>,
}

impl LibraryPage {
    pub fn new(settings: SharedSettings) -> Rc<Self> {
        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(14)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(32)
            .margin_end(32)
            .build();

        // Заголовок + meta.
        let title = gtk::Label::builder()
            .label("Библиотека")
            .halign(gtk::Align::Start)
            .build();
        title.add_css_class("title-1");
        container.append(&title);

        let meta_label = gtk::Label::builder()
            .label("—")
            .halign(gtk::Align::Start)
            .build();
        meta_label.add_css_class("dim-label");
        container.append(&meta_label);

        // Search + refresh.
        let top_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(4)
            .build();
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Поиск по названию…")
            .hexpand(true)
            .build();
        top_row.append(&search);
        let refresh_btn = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Обновить")
            .build();
        refresh_btn.add_css_class("flat");
        top_row.append(&refresh_btn);
        container.append(&top_row);

        // Сетка карточек.
        let flow = gtk::FlowBox::builder()
            .column_spacing(14)
            .row_spacing(14)
            .max_children_per_line(4)
            .min_children_per_line(1)
            .homogeneous(true)
            .selection_mode(gtk::SelectionMode::None)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&flow)
            .build();
        container.append(&scroll);

        let root: gtk::Widget = container.upcast();

        let this = Rc::new(Self {
            root,
            flow,
            search,
            meta_label,
            settings,
            on_open: Rc::new(RefCell::new(None)),
            items: Rc::new(RefCell::new(Vec::new())),
        });

        {
            let weak = Rc::downgrade(&this);
            refresh_btn.connect_clicked(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.refresh();
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            this.search.connect_search_changed(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.rebuild_flow();
                }
            });
        }

        this
    }

    pub fn set_on_open(&self, f: impl Fn(PathBuf) + 'static) {
        *self.on_open.borrow_mut() = Some(Box::new(f));
    }

    /// Текущий кеш записей — нужен shell.rs чтобы найти Recording по path.
    pub fn items_cache(&self) -> Vec<Recording> {
        self.items.borrow().clone()
    }

    /// Пересканировать директорию и перестроить сетку.
    pub fn refresh(self: &Rc<Self>) {
        let dir = self.settings.read().unwrap().output_dir.clone();
        let items = scan(&dir);
        let total_size: u64 = items.iter().map(|r| r.size_bytes).sum();
        self.meta_label.set_label(&format!(
            "{} записей · {}",
            items.len(),
            format_total_size(total_size)
        ));
        *self.items.borrow_mut() = items;
        self.rebuild_flow();
    }

    fn rebuild_flow(self: &Rc<Self>) {
        while let Some(child) = self.flow.first_child() {
            self.flow.remove(&child);
        }
        let query = self.search.text().to_string().to_lowercase();
        let items = self.items.borrow();
        for rec in items.iter() {
            if !query.is_empty() && !rec.title.to_lowercase().contains(&query) {
                continue;
            }
            let card = make_card(rec, self.on_open.clone());
            self.flow.append(&card);
        }
        if self.flow.first_child().is_none() {
            let empty = gtk::Label::builder()
                .label(if items.is_empty() {
                    "Записей пока нет. Завершённая запись появится здесь."
                } else {
                    "По запросу ничего не найдено."
                })
                .halign(gtk::Align::Center)
                .margin_top(40)
                .margin_bottom(40)
                .build();
            empty.add_css_class("dim-label");
            self.flow.append(&empty);
        }
    }
}

fn make_card(rec: &Recording, on_open: OpenCallback) -> gtk::Widget {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .width_request(220)
        .build();
    card.add_css_class("lib-card");

    // Thumbnail + overlay badges.
    let thumb_frame = gtk::AspectFrame::builder()
        .ratio(16.0 / 9.0)
        .obey_child(false)
        .build();
    let overlay = gtk::Overlay::new();

    let picture: gtk::Widget = match ensure_thumb(&rec.path) {
        Some(path) => {
            let pic = gtk::Picture::for_filename(&path);
            pic.set_can_shrink(true);
            pic.set_keep_aspect_ratio(true);
            pic.upcast()
        }
        None => {
            let placeholder = gtk::Label::new(Some("//"));
            placeholder.add_css_class("dim-label");
            placeholder.set_halign(gtk::Align::Center);
            placeholder.set_valign(gtk::Align::Center);
            let frame = gtk::Frame::new(None);
            frame.set_child(Some(&placeholder));
            frame.upcast()
        }
    };
    overlay.set_child(Some(&picture));

    if rec.has_transcript {
        let badge = gtk::Label::new(Some("AI"));
        badge.add_css_class("ai-badge");
        badge.set_halign(gtk::Align::End);
        badge.set_valign(gtk::Align::Start);
        badge.set_margin_top(6);
        badge.set_margin_end(6);
        overlay.add_overlay(&badge);
    }
    if rec.duration_seconds.is_some() {
        let dur = gtk::Label::new(Some(&rec.duration_display()));
        dur.add_css_class("duration-badge");
        dur.set_halign(gtk::Align::End);
        dur.set_valign(gtk::Align::End);
        dur.set_margin_bottom(6);
        dur.set_margin_end(6);
        overlay.add_overlay(&dur);
    }

    thumb_frame.set_child(Some(&overlay));
    card.append(&thumb_frame);

    // Текстовая часть.
    let title = gtk::Label::builder()
        .label(&rec.title)
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(30)
        .build();
    title.add_css_class("heading");
    card.append(&title);

    let meta_text = format!(
        "{} · {} · {}",
        rec.date_display(),
        rec.size_display(),
        rec.resolution_display()
    );
    let meta = gtk::Label::builder()
        .label(&meta_text)
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    meta.add_css_class("caption");
    meta.add_css_class("dim-label");
    card.append(&meta);

    // Клик по карточке → on_open.
    let click = gtk::GestureClick::new();
    let path = rec.path.clone();
    click.connect_released(move |_, n, _, _| {
        if n != 1 {
            return;
        }
        if let Some(cb) = on_open.borrow().as_ref() {
            cb(path.clone());
        }
    });
    card.add_controller(click);
    card.set_cursor_from_name(Some("pointer"));

    // Обернём в FlowBoxChild для ровных ячеек без selectable-outline.
    let fbc = gtk::FlowBoxChild::new();
    fbc.set_child(Some(&card));
    fbc.set_focusable(false);
    fbc.upcast()
}

fn format_total_size(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else {
        format!("{} KB", (bytes + 1023) / 1024)
    }
}

// Нужен, чтобы модуль мог использовать `glib` для сигналов (не используется напрямую,
// но сохраним для будущих расширений — например, debounce search).
#[allow(dead_code)]
fn _ensure_glib_linked() -> Option<glib::SourceId> {
    None
}
