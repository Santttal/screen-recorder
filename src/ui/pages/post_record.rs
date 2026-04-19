//! Экран «Запись готова». Phase 19.b.1 — показывается сразу после EOS
//! пайплайна. Редактируемый заголовок, превью (thumbnail + Play → внешний
//! плеер), статус-хинт Auto/Manual, кнопки Save/Export/Discard.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use libadwaita as adw;
use gtk4 as gtk;

use crate::config::SharedSettings;
use crate::library::ensure_thumb;

/// Решение пользователя по завершению.
pub enum PostOutcome {
    /// Сохранить (возможно с новым именем) → `final_path`.
    Saved(PathBuf),
    Discarded,
}

type OutcomeCallback = Rc<RefCell<Option<Box<dyn Fn(PostOutcome)>>>>;

#[allow(dead_code)]
pub struct PostRecordPage {
    pub root: gtk::Widget,
    preview_overlay: gtk::Overlay,
    title_entry: gtk::Entry,
    meta_label: gtk::Label,
    status_hint: gtk::Label,
    btn_save: gtk::Button,
    btn_export: gtk::Button,
    btn_discard: gtk::Button,
    current: Rc<RefCell<Option<PathBuf>>>,
    settings: SharedSettings,
    on_outcome: OutcomeCallback,
}

#[allow(dead_code)]
impl PostRecordPage {
    pub fn new(settings: SharedSettings) -> Rc<Self> {
        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(14)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(32)
            .margin_end(32)
            .build();

        // Badge + meta.
        let top_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .build();
        let badge_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .valign(gtk::Align::Center)
            .build();
        let check_icon = gtk::Image::from_icon_name("object-select-symbolic");
        check_icon.set_pixel_size(12);
        let badge_lbl = gtk::Label::new(Some("Записано"));
        badge_box.append(&check_icon);
        badge_box.append(&badge_lbl);
        badge_box.add_css_class("tag");
        badge_box.add_css_class("green");
        top_row.append(&badge_box);

        let meta_label = gtk::Label::builder()
            .label("")
            .halign(gtk::Align::Start)
            .build();
        meta_label.add_css_class("caption");
        meta_label.add_css_class("dim-label");
        top_row.append(&meta_label);
        container.append(&top_row);

        // Editable title.
        let title_entry = gtk::Entry::builder()
            .placeholder_text("Название записи")
            .build();
        title_entry.add_css_class("title-2");
        container.append(&title_entry);

        let caption = gtk::Label::builder()
            .label("Дайте имя и сохраните в библиотеку.")
            .halign(gtk::Align::Start)
            .build();
        caption.add_css_class("dim-label");
        container.append(&caption);

        // Preview.
        let preview_frame = gtk::AspectFrame::builder()
            .ratio(16.0 / 9.0)
            .obey_child(false)
            .build();
        let preview_overlay = gtk::Overlay::new();
        let placeholder = gtk::Frame::new(None);
        let placeholder_lbl = gtk::Label::new(Some("Превью"));
        placeholder_lbl.add_css_class("dim-label");
        placeholder.set_child(Some(&placeholder_lbl));
        preview_overlay.set_child(Some(&placeholder));

        let btn_play = gtk::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .tooltip_text("Открыть в системном плеере")
            .build();
        btn_play.add_css_class("circular");
        btn_play.add_css_class("suggested-action");
        preview_overlay.add_overlay(&btn_play);
        preview_frame.set_child(Some(&preview_overlay));
        container.append(&preview_frame);

        // Status hint (Auto / Manual).
        let status_hint = gtk::Label::builder()
            .label("")
            .halign(gtk::Align::Start)
            .margin_top(6)
            .build();
        status_hint.add_css_class("caption");
        status_hint.add_css_class("dim-label");
        container.append(&status_hint);

        // Actions.
        let actions = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .margin_top(10)
            .build();

        let btn_save = gtk::Button::builder().label("Сохранить в библиотеку").build();
        btn_save.add_css_class("suggested-action");
        btn_save.add_css_class("pill");
        actions.append(&btn_save);

        let btn_export = gtk::Button::builder().label("Экспорт…").build();
        btn_export.add_css_class("pill");
        actions.append(&btn_export);

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        actions.append(&spacer);

        let btn_discard = gtk::Button::builder()
            .label("Удалить")
            .build();
        btn_discard.add_css_class("destructive-action");
        btn_discard.add_css_class("pill");
        actions.append(&btn_discard);

        container.append(&actions);

        let root: gtk::Widget = container.upcast();

        let this = Rc::new(Self {
            root,
            preview_overlay,
            title_entry,
            meta_label,
            status_hint,
            btn_save,
            btn_export,
            btn_discard,
            current: Rc::new(RefCell::new(None)),
            settings,
            on_outcome: Rc::new(RefCell::new(None)),
        });

        // Play → xdg-open.
        {
            let current = this.current.clone();
            btn_play.connect_clicked(move |_| {
                let Some(p) = current.borrow().clone() else {
                    return;
                };
                let uri = format!("file://{}", p.display());
                let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
            });
        }

        // Save.
        {
            let weak = Rc::downgrade(&this);
            this.btn_save.connect_clicked(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.handle_save();
                }
            });
        }
        // Export (пока: открыть папку с файлом).
        {
            let current = this.current.clone();
            this.btn_export.connect_clicked(move |_| {
                let Some(p) = current.borrow().clone() else {
                    return;
                };
                if let Some(parent) = p.parent() {
                    let uri = format!("file://{}", parent.display());
                    let _ =
                        gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
                }
            });
        }
        // Discard.
        {
            let weak = Rc::downgrade(&this);
            this.btn_discard.connect_clicked(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.handle_discard();
                }
            });
        }

        this
    }

    pub fn set_on_outcome(&self, f: impl Fn(PostOutcome) + 'static) {
        *self.on_outcome.borrow_mut() = Some(Box::new(f));
    }

    /// Показать страницу для свежезаписанного файла.
    pub fn show(&self, path: PathBuf) {
        let file_name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Новая запись".to_owned());
        self.title_entry.set_text(&file_name);

        // Обновить meta (size + resolution if ffprobe).
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let (_, res) = ffprobe_duration_res(&path);
        let res_str = match res {
            Some((w, h)) => format!(" · {w}×{h}"),
            None => String::new(),
        };
        self.meta_label
            .set_label(&format!("Только что · {} MB{res_str}", size / 1_048_576));

        // Обновить превью.
        self.replace_preview(&path);

        // Обновить status hint по текущей настройке.
        let auto = self.settings.read().unwrap().transcription_enabled;
        self.status_hint.set_label(if auto {
            "После сохранения автоматически начнётся распознавание речи."
        } else {
            "Автораспознавание выключено. Запустите вручную из записи в Библиотеке."
        });

        *self.current.borrow_mut() = Some(path);
    }

    fn handle_save(&self) {
        let Some(orig_path) = self.current.borrow().clone() else {
            return;
        };
        let new_stem = self.title_entry.text().to_string();
        let new_stem = sanitize(&new_stem);
        let final_path = if new_stem.is_empty() || Some(new_stem.as_str()) == orig_path.file_stem().and_then(|s| s.to_str()) {
            orig_path.clone()
        } else {
            let ext = orig_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("mkv");
            let new_path = orig_path.with_file_name(format!("{new_stem}.{ext}"));
            if std::fs::rename(&orig_path, &new_path).is_err() {
                orig_path.clone()
            } else {
                // Если рядом есть .txt / .json — переименуем тоже.
                for ext in ["txt", "json"] {
                    let old_side = orig_path.with_extension(ext);
                    let new_side = new_path.with_extension(ext);
                    if old_side.is_file() {
                        let _ = std::fs::rename(&old_side, &new_side);
                    }
                }
                new_path
            }
        };
        if let Some(cb) = self.on_outcome.borrow().as_ref() {
            cb(PostOutcome::Saved(final_path));
        }
    }

    fn handle_discard(&self) {
        let Some(path) = self.current.borrow_mut().take() else {
            return;
        };
        let _ = std::fs::remove_file(&path);
        // Удалим и boundary-sidecars.
        for ext in ["txt", "json"] {
            let side = path.with_extension(ext);
            if side.is_file() {
                let _ = std::fs::remove_file(&side);
            }
        }
        if let Some(cb) = self.on_outcome.borrow().as_ref() {
            cb(PostOutcome::Discarded);
        }
    }

    fn replace_preview(&self, path: &Path) {
        self.preview_overlay.set_child(None::<&gtk::Widget>);
        let widget: gtk::Widget = match ensure_thumb(path) {
            Some(t) => {
                let pic = gtk::Picture::for_filename(&t);
                pic.set_can_shrink(true);
                pic.set_keep_aspect_ratio(true);
                pic.upcast()
            }
            None => {
                let placeholder = gtk::Label::new(Some("Превью недоступно"));
                placeholder.add_css_class("dim-label");
                let f = gtk::Frame::new(None);
                f.set_child(Some(&placeholder));
                f.upcast()
            }
        };
        self.preview_overlay.set_child(Some(&widget));
    }
}

fn sanitize(raw: &str) -> String {
    raw.trim()
        .replace(['/', '\\', '\0'], "_")
        .chars()
        .take(120)
        .collect()
}

fn ffprobe_duration_res(path: &Path) -> (Option<f64>, Option<(u32, u32)>) {
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v", "error", "-select_streams", "v:0",
            "-show_entries", "format=duration:stream=width,height",
            "-of", "default=noprint_wrappers=1",
        ])
        .arg(path)
        .output();
    let Ok(out) = out else {
        return (None, None);
    };
    if !out.status.success() {
        return (None, None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut dur = None;
    let mut w = None;
    let mut h = None;
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "duration" => dur = v.trim().parse().ok(),
                "width" => w = v.trim().parse().ok(),
                "height" => h = v.trim().parse().ok(),
                _ => {}
            }
        }
    }
    (dur, match (w, h) { (Some(w), Some(h)) => Some((w, h)), _ => None })
}
