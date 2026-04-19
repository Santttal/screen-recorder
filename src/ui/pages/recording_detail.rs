//! Экран «Recording detail» — плеер (внешний) + панель транскрипта.
//! Phase 19.b.5 — каркас; wiring STT-команд и рендер сегментов — 19.b.6/19.b.7.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use libadwaita as adw;
use gtk4 as gtk;

use crate::library::{ensure_thumb, Recording};

/// Состояние транскрипт-панели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum TranscriptState {
    None,
    Processing,
    Done,
}

type PathCallback = Rc<RefCell<Option<Box<dyn Fn(PathBuf)>>>>;
type VoidCallback = Rc<RefCell<Option<Box<dyn Fn()>>>>;

#[allow(dead_code)]
pub struct RecordingDetailPage {
    pub root: gtk::Widget,

    // Top bar
    btn_back: gtk::Button,
    title_label: gtk::Label,
    sub_label: gtk::Label,
    btn_export: gtk::MenuButton,
    btn_more: gtk::MenuButton,

    // Player column
    preview_overlay: gtk::Overlay,
    btn_play: gtk::Button,
    meta_row_source: adw::ActionRow,
    meta_row_audio: adw::ActionRow,
    meta_row_path: adw::ActionRow,

    // Transcript column
    transcript_stack: gtk::Stack, // children: "empty", "processing", "done"
    btn_transcribe: gtk::Button,
    progress_bar: gtk::ProgressBar,
    transcript_list: gtk::ListBox,
    model_badge: gtk::Label,

    // Current recording + callbacks
    current: Rc<RefCell<Option<Recording>>>,
    on_back: VoidCallback,
    on_transcribe: PathCallback,
}

#[allow(dead_code)]
impl RecordingDetailPage {
    pub fn new() -> Rc<Self> {
        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(0)
            .build();

        // ---------- Top bar ----------
        let topbar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(14)
            .margin_end(14)
            .build();

        let btn_back = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text("Назад в библиотеку")
            .build();
        btn_back.add_css_class("flat");
        topbar.append(&btn_back);

        let title_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(1)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .margin_start(6)
            .build();
        let title_label = gtk::Label::builder()
            .label("—")
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        title_label.add_css_class("heading");
        let sub_label = gtk::Label::builder()
            .label("")
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        sub_label.add_css_class("caption");
        sub_label.add_css_class("dim-label");
        title_box.append(&title_label);
        title_box.append(&sub_label);
        topbar.append(&title_box);

        // Export — popover с вариантами Copy / TXT / SRT / VTT (phase 19.b.9).
        let btn_export = gtk::MenuButton::builder()
            .icon_name("document-save-as-symbolic")
            .tooltip_text("Экспорт…")
            .build();
        btn_export.add_css_class("flat");
        let export_popover = gtk::Popover::new();
        let export_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        let btn_copy = flat_popover_button("edit-copy-symbolic", "Копировать текст");
        let btn_txt = flat_popover_button("document-save-symbolic", "Сохранить как .txt");
        let btn_srt = flat_popover_button("document-save-symbolic", "Сохранить как .srt");
        let btn_vtt = flat_popover_button("document-save-symbolic", "Сохранить как .vtt");
        export_box.append(&btn_copy);
        export_box.append(&btn_txt);
        export_box.append(&btn_srt);
        export_box.append(&btn_vtt);
        export_popover.set_child(Some(&export_box));
        btn_export.set_popover(Some(&export_popover));
        topbar.append(&btn_export);

        let btn_more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("Ещё")
            .build();
        btn_more.add_css_class("flat");
        // More popover (phase 19.b.10).
        let more_popover = gtk::Popover::new();
        let more_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        let btn_show_in_files = flat_popover_button("folder-symbolic", "Показать в файлах");
        let btn_delete = flat_popover_button("user-trash-symbolic", "Удалить запись");
        btn_delete.add_css_class("destructive-action");
        more_box.append(&btn_show_in_files);
        more_box.append(&btn_delete);
        more_popover.set_child(Some(&more_box));
        btn_more.set_popover(Some(&more_popover));
        topbar.append(&btn_more);

        container.append(&topbar);
        container.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ---------- Body: Paned(player | transcript) ----------
        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .wide_handle(true)
            .hexpand(true)
            .vexpand(true)
            .build();

        // --- Player column ---
        let player_col = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .margin_top(16)
            .margin_bottom(16)
            .margin_start(20)
            .margin_end(20)
            .build();

        let preview_frame = gtk::AspectFrame::builder()
            .ratio(16.0 / 9.0)
            .obey_child(false)
            .build();
        let preview_overlay = gtk::Overlay::new();
        let preview_placeholder = gtk::Frame::new(None);
        let placeholder_lbl = gtk::Label::new(Some("Нет превью"));
        placeholder_lbl.add_css_class("dim-label");
        preview_placeholder.set_child(Some(&placeholder_lbl));
        preview_overlay.set_child(Some(&preview_placeholder));

        let btn_play = gtk::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        btn_play.add_css_class("circular");
        btn_play.add_css_class("suggested-action");
        btn_play.set_tooltip_text(Some("Открыть в системном плеере"));
        preview_overlay.add_overlay(&btn_play);
        preview_frame.set_child(Some(&preview_overlay));
        player_col.append(&preview_frame);

        // Meta rows.
        let meta_group = adw::PreferencesGroup::new();
        let meta_row_source = make_meta_row("Источник", "—");
        let meta_row_audio = make_meta_row("Аудио", "—");
        let meta_row_path = make_meta_row("Сохранено", "—");
        meta_group.add(&meta_row_source);
        meta_group.add(&meta_row_audio);
        meta_group.add(&meta_row_path);
        player_col.append(&meta_group);

        let player_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&player_col)
            .build();
        paned.set_start_child(Some(&player_scroll));

        // --- Transcript column ---
        let transcript_col = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(0)
            .build();

        let tr_header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(16)
            .margin_end(16)
            .build();
        let tr_icon = gtk::Image::from_icon_name("text-x-generic-symbolic");
        tr_icon.add_css_class("dim-label");
        tr_header.append(&tr_icon);
        let tr_title = gtk::Label::new(Some("Расшифровка"));
        tr_title.add_css_class("heading");
        tr_header.append(&tr_title);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        tr_header.append(&spacer);
        let model_badge = gtk::Label::new(Some("OpenAI"));
        model_badge.add_css_class("tag");
        model_badge.add_css_class("blue");
        model_badge.set_visible(false);
        tr_header.append(&model_badge);
        transcript_col.append(&tr_header);
        transcript_col.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // Stack: empty / processing / done.
        let transcript_stack = gtk::Stack::builder().vexpand(true).hexpand(true).build();

        // Empty state.
        let empty = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .margin_top(40)
            .margin_bottom(40)
            .margin_start(32)
            .margin_end(32)
            .build();
        let empty_icon = gtk::Image::from_icon_name("text-x-generic-symbolic");
        empty_icon.set_pixel_size(48);
        empty_icon.add_css_class("dim-label");
        empty.append(&empty_icon);
        let empty_title = gtk::Label::new(Some("Расшифровки пока нет"));
        empty_title.add_css_class("heading");
        empty.append(&empty_title);
        let empty_sub = gtk::Label::new(Some(
            "Сгенерируйте таймкодированный транскрипт через OpenAI. Обычно занимает 30–60 секунд.",
        ));
        empty_sub.set_wrap(true);
        empty_sub.set_justify(gtk::Justification::Center);
        empty_sub.set_max_width_chars(40);
        empty_sub.add_css_class("dim-label");
        empty.append(&empty_sub);
        let btn_transcribe = gtk::Button::builder()
            .label("Распознать речь")
            .halign(gtk::Align::Center)
            .build();
        btn_transcribe.add_css_class("suggested-action");
        btn_transcribe.add_css_class("pill");
        empty.append(&btn_transcribe);
        transcript_stack.add_named(&empty, Some("empty"));

        // Processing state.
        let processing = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .margin_top(40)
            .margin_bottom(40)
            .build();
        let spinner = gtk::Spinner::new();
        spinner.set_spinning(true);
        spinner.set_width_request(32);
        spinner.set_height_request(32);
        processing.append(&spinner);
        let proc_title = gtk::Label::new(Some("Распознаю речь…"));
        proc_title.add_css_class("heading");
        processing.append(&proc_title);
        let progress_bar = gtk::ProgressBar::builder().width_request(240).build();
        processing.append(&progress_bar);
        transcript_stack.add_named(&processing, Some("processing"));

        // Done state.
        let done_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(0)
            .build();

        // Поиск по транскрипту (phase 19.b.8).
        let search_bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(8)
            .margin_bottom(4)
            .margin_start(14)
            .margin_end(14)
            .build();
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Найти в транскрипте…")
            .hexpand(true)
            .build();
        search_bar.append(&search_entry);
        done_box.append(&search_bar);

        let transcript_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        transcript_list.add_css_class("boxed-list");
        let done_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&transcript_list)
            .margin_top(4)
            .margin_bottom(10)
            .margin_start(12)
            .margin_end(12)
            .build();
        done_box.append(&done_scroll);
        transcript_stack.add_named(&done_box, Some("done"));

        // Связь filter-func со SearchEntry.
        {
            let list = transcript_list.clone();
            search_entry.connect_search_changed(move |entry| {
                let q = entry.text().to_string().to_lowercase();
                let mut idx = 0;
                while let Some(row) = list.row_at_index(idx) {
                    let visible = if q.is_empty() {
                        true
                    } else {
                        row_text(&row).to_lowercase().contains(&q)
                    };
                    row.set_visible(visible);
                    idx += 1;
                }
            });
        }

        transcript_stack.set_visible_child_name("empty");
        transcript_col.append(&transcript_stack);
        paned.set_end_child(Some(&transcript_col));
        paned.set_position(520);

        container.append(&paned);

        let root: gtk::Widget = container.upcast();

        let this = Rc::new(Self {
            root,
            btn_back,
            title_label,
            sub_label,
            btn_export,
            btn_more,
            preview_overlay,
            btn_play,
            meta_row_source,
            meta_row_audio,
            meta_row_path,
            transcript_stack,
            btn_transcribe,
            progress_bar,
            transcript_list,
            model_badge,
            current: Rc::new(RefCell::new(None)),
            on_back: Rc::new(RefCell::new(None)),
            on_transcribe: Rc::new(RefCell::new(None)),
        });

        // Wire Back.
        {
            let cb = this.on_back.clone();
            this.btn_back.connect_clicked(move |_| {
                if let Some(f) = cb.borrow().as_ref() {
                    f();
                }
            });
        }
        // Wire Play → xdg-open.
        {
            let current = this.current.clone();
            this.btn_play.connect_clicked(move |_| {
                if let Some(rec) = current.borrow().as_ref() {
                    let uri = format!("file://{}", rec.path.display());
                    if let Err(e) =
                        gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE)
                    {
                        tracing::warn!(%e, "failed to open recording externally");
                    }
                }
            });
        }
        // Wire Transcribe — вызов добавляется в 19.b.6.
        {
            let cb = this.on_transcribe.clone();
            let current = this.current.clone();
            this.btn_transcribe.connect_clicked(move |_| {
                let Some(rec) = current.borrow().clone() else {
                    return;
                };
                if let Some(f) = cb.borrow().as_ref() {
                    f(rec.path);
                }
            });
        }

        // Wire More → Show in files (phase 19.b.10).
        {
            let current = this.current.clone();
            btn_show_in_files.connect_clicked(move |_| {
                let Some(rec) = current.borrow().clone() else {
                    return;
                };
                if let Some(parent) = rec.path.parent() {
                    let uri = format!("file://{}", parent.display());
                    let _ =
                        gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
                }
            });
        }
        // Wire More → Delete with confirm (phase 19.b.10).
        {
            let current = this.current.clone();
            let on_back = this.on_back.clone();
            let root_weak = this.root.downgrade();
            btn_delete.connect_clicked(move |_| {
                let Some(rec) = current.borrow().clone() else {
                    return;
                };
                let parent_win = root_weak
                    .upgrade()
                    .and_then(|w| w.root())
                    .and_then(|r| r.downcast::<gtk::Window>().ok());
                let dialog = gtk::MessageDialog::builder()
                    .modal(true)
                    .message_type(gtk::MessageType::Warning)
                    .buttons(gtk::ButtonsType::None)
                    .text("Удалить запись?")
                    .secondary_text(&format!(
                        "Файл «{}» и связанные транскрипты будут удалены безвозвратно.",
                        rec.title
                    ))
                    .build();
                dialog.add_button("Отмена", gtk::ResponseType::Cancel);
                dialog.add_button("Удалить", gtk::ResponseType::Accept);
                dialog.set_default_response(gtk::ResponseType::Cancel);
                if let Some(w) = parent_win.as_ref() {
                    dialog.set_transient_for(Some(w));
                }
                let path = rec.path.clone();
                let on_back = on_back.clone();
                dialog.connect_response(move |d, response| {
                    d.close();
                    if response != gtk::ResponseType::Accept {
                        return;
                    }
                    let _ = std::fs::remove_file(&path);
                    for ext in ["txt", "json", "srt", "vtt"] {
                        let side = path.with_extension(ext);
                        if side.is_file() {
                            let _ = std::fs::remove_file(&side);
                        }
                    }
                    // Удалить thumbnail из кеша тоже.
                    let thumb = crate::library::thumb_path(&path);
                    let _ = std::fs::remove_file(&thumb);
                    if let Some(f) = on_back.borrow().as_ref() {
                        f();
                    }
                });
                dialog.present();
            });
        }

        // Wire Export buttons (phase 19.b.9).
        {
            let current = this.current.clone();
            btn_copy.connect_clicked(move |_| {
                let Some(rec) = current.borrow().clone() else {
                    return;
                };
                let txt = read_plain_text(&rec.path);
                if let Some(d) = gtk::gdk::Display::default() {
                    d.clipboard().set_text(&txt);
                }
                export_popover.popdown();
            });
        }
        {
            let current = this.current.clone();
            btn_txt.connect_clicked(move |_| {
                let Some(rec) = current.borrow().clone() else {
                    return;
                };
                let src = rec.path.with_extension("txt");
                let dst = rec.path.with_extension("txt.export.txt");
                if src.is_file() {
                    let _ = std::fs::copy(&src, &dst);
                    open_file(&dst);
                }
            });
        }
        {
            let current = this.current.clone();
            btn_srt.connect_clicked(move |_| {
                let Some(rec) = current.borrow().clone() else {
                    return;
                };
                let segments = load_segments(&rec.path);
                let srt = to_srt(&segments);
                let path = rec.path.with_extension("srt");
                if std::fs::write(&path, srt).is_ok() {
                    open_file(&path);
                }
            });
        }
        {
            let current = this.current.clone();
            btn_vtt.connect_clicked(move |_| {
                let Some(rec) = current.borrow().clone() else {
                    return;
                };
                let segments = load_segments(&rec.path);
                let vtt = to_vtt(&segments);
                let path = rec.path.with_extension("vtt");
                if std::fs::write(&path, vtt).is_ok() {
                    open_file(&path);
                }
            });
        }

        this
    }

    pub fn set_on_back(&self, f: impl Fn() + 'static) {
        *self.on_back.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_on_transcribe(&self, f: impl Fn(PathBuf) + 'static) {
        *self.on_transcribe.borrow_mut() = Some(Box::new(f));
    }

    /// Показать конкретную запись. Обновляет topbar, превью, meta, transcript-state.
    pub fn show_recording(&self, rec: Recording) {
        // Topbar.
        self.title_label.set_label(&rec.title);
        let meta = format!(
            "{} · {} · {} · {}",
            rec.date_display(),
            rec.duration_display(),
            rec.size_display(),
            rec.resolution_display()
        );
        self.sub_label.set_label(&meta);

        // Превью.
        self.replace_preview(&rec.path);

        // Meta rows.
        self.meta_row_source.set_subtitle("Весь экран · portal");
        self.meta_row_audio.set_subtitle("Микрофон + Системный звук");
        self.meta_row_path.set_subtitle(&rec.path.display().to_string());

        // Transcript state.
        if rec.has_transcript {
            self.set_transcript_state(TranscriptState::Done);
            self.populate_transcript_from_txt(&rec.path);
        } else {
            self.set_transcript_state(TranscriptState::None);
        }

        *self.current.borrow_mut() = Some(rec);
    }

    pub fn set_transcript_state(&self, st: TranscriptState) {
        match st {
            TranscriptState::None => {
                self.transcript_stack.set_visible_child_name("empty");
                self.model_badge.set_visible(false);
            }
            TranscriptState::Processing => {
                self.transcript_stack.set_visible_child_name("processing");
                self.progress_bar.set_fraction(0.0);
            }
            TranscriptState::Done => {
                self.transcript_stack.set_visible_child_name("done");
                self.model_badge.set_visible(true);
            }
        }
    }

    pub fn set_transcript_progress(&self, fraction: f64) {
        self.progress_bar.set_fraction(fraction.clamp(0.0, 1.0));
    }

    /// Если detail сейчас открыт на `video_path` — перечитать .txt и перейти в Done.
    pub fn reload_if_current(&self, video_path: &Path) {
        let same = self
            .current
            .borrow()
            .as_ref()
            .map(|r| r.path == video_path)
            .unwrap_or(false);
        if !same {
            return;
        }
        self.populate_transcript_from_txt(video_path);
        self.set_transcript_state(TranscriptState::Done);
    }

    /// Показать транскрипт: сначала пытаемся .json с сегментами (phase 19.b.7),
    /// в fallback — plain .txt одним блоком.
    fn populate_transcript_from_txt(&self, video_path: &Path) {
        while let Some(child) = self.transcript_list.first_child() {
            self.transcript_list.remove(&child);
        }

        // 1) .json с сегментами (whisper-1 verbose_json / diarize).
        let json_path = video_path.with_extension("json");
        if let Ok(raw) = std::fs::read_to_string(&json_path) {
            if let Ok(segments) = serde_json::from_str::<Vec<crate::transcription::client::Segment>>(&raw) {
                if !segments.is_empty() {
                    for seg in &segments {
                        self.transcript_list.append(&make_segment_row(seg));
                    }
                    return;
                }
            }
        }

        // 2) Fallback — plain .txt.
        let txt_path = video_path.with_extension("txt");
        let Ok(text) = std::fs::read_to_string(&txt_path) else {
            return;
        };
        let row = gtk::ListBoxRow::new();
        let label = gtk::Label::builder()
            .label(&text)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .halign(gtk::Align::Start)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(14)
            .margin_end(14)
            .selectable(true)
            .build();
        row.set_child(Some(&label));
        self.transcript_list.append(&row);
    }

    fn replace_preview(&self, video_path: &Path) {
        if let Some(child) = self.preview_overlay.child() {
            self.preview_overlay.set_child(None::<&gtk::Widget>);
            let _ = child;
        }
        let widget: gtk::Widget = match ensure_thumb(video_path) {
            Some(thumb) => {
                let pic = gtk::Picture::for_filename(&thumb);
                pic.set_can_shrink(true);
                pic.set_keep_aspect_ratio(true);
                pic.upcast()
            }
            None => {
                let placeholder = gtk::Label::new(Some("Превью недоступно"));
                placeholder.add_css_class("dim-label");
                let frame = gtk::Frame::new(None);
                frame.set_child(Some(&placeholder));
                frame.upcast()
            }
        };
        self.preview_overlay.set_child(Some(&widget));
        // Re-add the play button as overlay (overlay children persist through
        // child replacement, но для надёжности сделаем повторно).
        self.btn_play.set_visible(true);
    }
}

fn make_meta_row(key: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(key).subtitle(value).build();
    row.add_css_class("property");
    row
}

fn make_segment_row(seg: &crate::transcription::client::Segment) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("transcript-row");
    let hbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .build();

    let time_lbl = gtk::Label::builder()
        .label(&format_timestamp(seg.start))
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .width_chars(6)
        .build();
    time_lbl.add_css_class("transcript-time");
    hbox.append(&time_lbl);

    if let Some(speaker) = &seg.speaker {
        let spk_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .valign(gtk::Align::Start)
            .build();
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.set_size_request(8, 8);
        dot.add_css_class("speaker-dot");
        if speaker.ends_with('1') || speaker.ends_with("B") {
            dot.add_css_class("b");
        }
        spk_box.append(&dot);
        let lbl = gtk::Label::new(Some(&pretty_speaker(speaker)));
        lbl.add_css_class("caption");
        lbl.add_css_class("dim-label");
        spk_box.append(&lbl);
        hbox.append(&spk_box);
    }

    let text_lbl = gtk::Label::builder()
        .label(&seg.text)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .hexpand(true)
        .selectable(true)
        .build();
    hbox.append(&text_lbl);

    row.set_child(Some(&hbox));
    row
}

/// Собрать весь видимый текст из листовых `gtk::Label` внутри row.
/// Нужно для фильтрации поиска (phase 19.b.8).
fn row_text(row: &gtk::ListBoxRow) -> String {
    let mut out = String::new();
    collect_label_text(&row.clone().upcast::<gtk::Widget>(), &mut out);
    out
}

fn collect_label_text(widget: &gtk::Widget, out: &mut String) {
    if let Ok(lbl) = widget.clone().downcast::<gtk::Label>() {
        out.push(' ');
        out.push_str(&lbl.text());
        return;
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        collect_label_text(&c, out);
        child = c.next_sibling();
    }
}

fn format_timestamp(secs: f64) -> String {
    let total = secs as i64;
    let m = total / 60;
    let s = total % 60;
    format!("{m:02}:{s:02}")
}

fn pretty_speaker(raw: &str) -> String {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    match digits.parse::<u32>() {
        Ok(n) => format!("Speaker {}", (b'A' + n.min(25) as u8) as char),
        Err(_) => raw.to_owned(),
    }
}

fn flat_popover_button(icon: &str, label: &str) -> gtk::Button {
    let btn = gtk::Button::builder().build();
    btn.add_css_class("flat");
    let hbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_top(4)
        .margin_bottom(4)
        .margin_start(4)
        .margin_end(4)
        .build();
    let ic = gtk::Image::from_icon_name(icon);
    hbox.append(&ic);
    let lbl = gtk::Label::new(Some(label));
    lbl.set_halign(gtk::Align::Start);
    lbl.set_hexpand(true);
    hbox.append(&lbl);
    btn.set_child(Some(&hbox));
    btn
}

fn read_plain_text(video_path: &Path) -> String {
    std::fs::read_to_string(video_path.with_extension("txt")).unwrap_or_default()
}

fn load_segments(video_path: &Path) -> Vec<crate::transcription::client::Segment> {
    let json_path = video_path.with_extension("json");
    if let Ok(raw) = std::fs::read_to_string(&json_path) {
        if let Ok(segs) =
            serde_json::from_str::<Vec<crate::transcription::client::Segment>>(&raw)
        {
            return segs;
        }
    }
    Vec::new()
}

fn to_srt(segments: &[crate::transcription::client::Segment]) -> String {
    let mut out = String::new();
    for (i, seg) in segments.iter().enumerate() {
        out.push_str(&format!("{}\n", i + 1));
        out.push_str(&format!(
            "{} --> {}\n",
            srt_timestamp(seg.start),
            srt_timestamp(seg.end)
        ));
        out.push_str(seg.text.trim());
        out.push_str("\n\n");
    }
    out
}

fn to_vtt(segments: &[crate::transcription::client::Segment]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for seg in segments.iter() {
        out.push_str(&format!(
            "{} --> {}\n",
            vtt_timestamp(seg.start),
            vtt_timestamp(seg.end)
        ));
        out.push_str(seg.text.trim());
        out.push_str("\n\n");
    }
    out
}

fn srt_timestamp(secs: f64) -> String {
    let total = secs;
    let h = (total as i64) / 3600;
    let m = ((total as i64) % 3600) / 60;
    let s = (total as i64) % 60;
    let ms = ((total - total.floor()) * 1000.0) as i64;
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}

fn vtt_timestamp(secs: f64) -> String {
    let total = secs;
    let h = (total as i64) / 3600;
    let m = ((total as i64) % 3600) / 60;
    let s = (total as i64) % 60;
    let ms = ((total - total.floor()) * 1000.0) as i64;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

fn open_file(path: &Path) {
    let uri = format!("file://{}", path.display());
    let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
}
