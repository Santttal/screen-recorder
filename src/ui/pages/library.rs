//! Экран «Библиотека» (phase 19.b.4 + fixes после UAT).
//!
//! Быстрый открытие: первичный `scan` (read_dir + metadata) + render с
//! placeholders → фоновый thread делает ffprobe + ensure_thumb для каждой
//! записи → апдейты приходят в GTK-поток через `async_channel`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::thread;

use adw::prelude::*;
use gtk::glib;
use libadwaita as adw;
use gtk4 as gtk;

use crate::config::SharedSettings;
use crate::library::{enrich, ensure_thumb, scan, thumb_path, Recording};

type OpenCallback = Rc<RefCell<Option<Box<dyn Fn(PathBuf)>>>>;

/// Обновление для конкретной карточки, которое приходит из фонового потока.
enum CardUpdate {
    /// ffprobe-результат: duration + resolution.
    Meta {
        path: PathBuf,
        duration_seconds: Option<f64>,
        resolution: Option<(u32, u32)>,
    },
    /// Thumbnail готов.
    Thumb { path: PathBuf, thumb: PathBuf },
}

struct CardHandles {
    picture_slot: gtk::Box,   // контейнер, куда кладётся Picture или плейсхолдер
    duration_badge: gtk::Label,
    meta_label: gtk::Label,
}

pub struct LibraryPage {
    pub root: gtk::Widget,
    flow: gtk::FlowBox,
    search: gtk::SearchEntry,
    meta_label: gtk::Label,
    settings: SharedSettings,
    on_open: OpenCallback,
    items: Rc<RefCell<Vec<Recording>>>,
    cards: Rc<RefCell<HashMap<PathBuf, CardHandles>>>,
    update_tx: async_channel::Sender<CardUpdate>,
}

impl LibraryPage {
    pub fn new(settings: SharedSettings) -> Rc<Self> {
        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(32)
            .margin_end(32)
            .build();

        // Верхняя строка: слева title + meta; справа search + refresh.
        let top = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .build();

        let title_col = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(1)
            .hexpand(true)
            .build();
        let title = gtk::Label::builder()
            .label("Библиотека")
            .halign(gtk::Align::Start)
            .build();
        title.add_css_class("title-1");
        let meta_label = gtk::Label::builder()
            .label("—")
            .halign(gtk::Align::Start)
            .build();
        meta_label.add_css_class("dim-label");
        title_col.append(&title);
        title_col.append(&meta_label);
        top.append(&title_col);

        let search = gtk::SearchEntry::builder()
            .placeholder_text("Поиск записей…")
            .width_chars(24)
            .valign(gtk::Align::End)
            .build();
        top.append(&search);
        let refresh_btn = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Обновить")
            .valign(gtk::Align::End)
            .build();
        refresh_btn.add_css_class("flat");
        top.append(&refresh_btn);
        container.append(&top);

        // Сетка карточек. Homogeneous выключен — FlowBox не растягивает карточки
        // на всю ширину viewport; фиксированный размер задаём в make_card.
        let flow = gtk::FlowBox::builder()
            .column_spacing(18)
            .row_spacing(18)
            .max_children_per_line(4)
            .min_children_per_line(1)
            .homogeneous(false)
            .selection_mode(gtk::SelectionMode::None)
            .halign(gtk::Align::Start)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&flow)
            .build();
        container.append(&scroll);

        let root: gtk::Widget = container.upcast();

        let (update_tx, update_rx) = async_channel::unbounded::<CardUpdate>();

        let this = Rc::new(Self {
            root,
            flow,
            search,
            meta_label,
            settings,
            on_open: Rc::new(RefCell::new(None)),
            items: Rc::new(RefCell::new(Vec::new())),
            cards: Rc::new(RefCell::new(HashMap::new())),
            update_tx,
        });

        // GTK-thread обработчик апдейтов.
        {
            let weak = Rc::downgrade(&this);
            glib::MainContext::default().spawn_local(async move {
                while let Ok(update) = update_rx.recv().await {
                    let Some(me) = weak.upgrade() else { return };
                    me.apply_update(update);
                }
            });
        }

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

    pub fn items_cache(&self) -> Vec<Recording> {
        self.items.borrow().clone()
    }

    /// Пересканировать (быстро) и перестроить сетку. Тяжёлое обогащение — в фоне.
    pub fn refresh(self: &Rc<Self>) {
        let dir = self.settings.read().unwrap().output_dir.clone();
        let items = scan(&dir);
        let total_size: u64 = items.iter().map(|r| r.size_bytes).sum();
        self.meta_label.set_label(&format!(
            "{} записей · {}",
            items.len(),
            format_total_size(total_size)
        ));
        *self.items.borrow_mut() = items.clone();
        self.rebuild_flow();

        // Фоновая задача: для каждой записи — ffprobe + ensure_thumb.
        let tx = self.update_tx.clone();
        thread::spawn(move || {
            for rec in items {
                // Thumbnail: если уже в кеше — быстро; иначе ffmpeg.
                if let Some(t) = ensure_thumb(&rec.path) {
                    let _ = tx.send_blocking(CardUpdate::Thumb {
                        path: rec.path.clone(),
                        thumb: t,
                    });
                }
                // ffprobe: duration + resolution.
                let (d, r) = enrich(&rec.path);
                let _ = tx.send_blocking(CardUpdate::Meta {
                    path: rec.path.clone(),
                    duration_seconds: d,
                    resolution: r,
                });
            }
        });
    }

    fn apply_update(&self, update: CardUpdate) {
        let cards = self.cards.borrow();
        match update {
            CardUpdate::Meta {
                path,
                duration_seconds,
                resolution,
            } => {
                // Обновить кеш items.
                {
                    let mut items = self.items.borrow_mut();
                    if let Some(rec) = items.iter_mut().find(|r| r.path == path) {
                        rec.duration_seconds = duration_seconds;
                        rec.resolution = resolution;
                    }
                }
                if let Some(h) = cards.get(&path) {
                    // Найти карточку и обновить duration-badge + meta.
                    let items = self.items.borrow();
                    if let Some(rec) = items.iter().find(|r| r.path == path) {
                        if rec.duration_seconds.is_some() {
                            h.duration_badge.set_label(&rec.duration_display());
                            h.duration_badge.set_visible(true);
                        }
                        h.meta_label.set_label(&format!(
                            "{} · {} · {}",
                            rec.date_display(),
                            rec.size_display(),
                            rec.resolution_display()
                        ));
                    }
                }
            }
            CardUpdate::Thumb { path, thumb } => {
                if let Some(h) = cards.get(&path) {
                    // Заменить содержимое picture_slot.
                    while let Some(c) = h.picture_slot.first_child() {
                        h.picture_slot.remove(&c);
                    }
                    let pic = gtk::Picture::for_filename(&thumb);
                    pic.set_can_shrink(true);
                    pic.set_keep_aspect_ratio(true);
                    pic.set_hexpand(true);
                    pic.set_vexpand(true);
                    h.picture_slot.append(&pic);
                }
            }
        }
    }

    fn rebuild_flow(self: &Rc<Self>) {
        while let Some(child) = self.flow.first_child() {
            self.flow.remove(&child);
        }
        self.cards.borrow_mut().clear();

        let query = self.search.text().to_string().to_lowercase();
        let items = self.items.borrow();
        let mut any = false;
        for rec in items.iter() {
            if !query.is_empty() && !rec.title.to_lowercase().contains(&query) {
                continue;
            }
            any = true;
            let (fbc, handles) = make_card(rec, self.on_open.clone());
            self.flow.append(&fbc);
            self.cards.borrow_mut().insert(rec.path.clone(), handles);
        }
        if !any {
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

fn make_card(rec: &Recording, on_open: OpenCallback) -> (gtk::FlowBoxChild, CardHandles) {
    // Фиксированный размер картиночной области — 220×124 (≈16:9).
    // Ширину подобрали так, чтобы при окне 960×640 и sidebar 200px в ряд
    // помещалось 3 карточки: 220*3 + spacing 18*2 + padding 32*2 ≈ 760 < 760px.
    const THUMB_W: i32 = 220;
    const THUMB_H: i32 = 124;

    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .build();
    card.add_css_class("lib-card");
    card.set_size_request(THUMB_W, -1);

    // Overlay с фиксированным размером — содержит Picture/placeholder + badges.
    let overlay = gtk::Overlay::new();
    overlay.add_css_class("lib-card-thumb");
    overlay.set_size_request(THUMB_W, THUMB_H);

    // picture_slot — Box, куда кладём либо Picture (когда thumb готов), либо placeholder.
    let picture_slot = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .vexpand(true)
        .build();
    picture_slot.set_size_request(THUMB_W, THUMB_H);

    let existing = thumb_path(&rec.path);
    if existing.is_file() {
        let pic = gtk::Picture::for_filename(&existing);
        pic.set_can_shrink(true);
        pic.set_keep_aspect_ratio(true);
        pic.set_hexpand(true);
        pic.set_vexpand(true);
        picture_slot.append(&pic);
    } else {
        // Короткий placeholder — не даём длинному имени раздуть карточку.
        let placeholder = gtk::Label::new(Some("…"));
        placeholder.add_css_class("dim-label");
        placeholder.set_halign(gtk::Align::Center);
        placeholder.set_valign(gtk::Align::Center);
        placeholder.set_hexpand(true);
        placeholder.set_vexpand(true);
        picture_slot.append(&placeholder);
    }
    overlay.set_child(Some(&picture_slot));

    // AI badge (top-left).
    let ai_badge_visible = rec.has_transcript;
    let ai_badge = gtk::Label::new(Some("✦ AI"));
    ai_badge.add_css_class("ai-badge");
    ai_badge.set_halign(gtk::Align::Start);
    ai_badge.set_valign(gtk::Align::Start);
    ai_badge.set_margin_top(8);
    ai_badge.set_margin_start(8);
    ai_badge.set_visible(ai_badge_visible);
    overlay.add_overlay(&ai_badge);

    // Duration badge (bottom-right).
    let duration_text = rec.duration_display();
    let duration_badge = gtk::Label::new(Some(&duration_text));
    duration_badge.add_css_class("duration-badge");
    duration_badge.set_halign(gtk::Align::End);
    duration_badge.set_valign(gtk::Align::End);
    duration_badge.set_margin_bottom(8);
    duration_badge.set_margin_end(8);
    duration_badge.set_visible(rec.duration_seconds.is_some());
    overlay.add_overlay(&duration_badge);

    card.append(&overlay);

    // ── Текстовая часть (внутри карточки, padding).
    let info = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(10)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    let title = gtk::Label::builder()
        .label(&rec.title)
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(28)
        .build();
    title.add_css_class("heading");
    info.append(&title);

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
    info.append(&meta);
    card.append(&info);

    // Клик → on_open.
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

    let fbc = gtk::FlowBoxChild::new();
    fbc.set_child(Some(&card));
    fbc.set_focusable(false);
    fbc.set_halign(gtk::Align::Start);
    fbc.set_hexpand(false);

    let handles = CardHandles {
        picture_slot,
        duration_badge,
        meta_label: meta,
    };
    (fbc, handles)
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
