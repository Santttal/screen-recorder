//! Экран «Настройки». Phase 19.a.6 — переезд из отдельного
//! `adw::PreferencesWindow` в in-app Stack child.
//!
//! Все группы складываются в единый `adw::PreferencesPage` —
//! он сам делает скроллинг и отступы. UX: одна длинная страница вместо
//! нескольких вкладок (как в финальном дизайне Ralume AI).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use libadwaita as adw;
use gtk4 as gtk;

use crate::config::{
    AudioMode, CaptureSource, Container, CursorMode, EncoderHint, Settings, SharedSettings,
    TranscriptionModel, VideoCodec,
};
use crate::recorder::encoders::detect_available_encoders;

const SAVE_DEBOUNCE_MS: u64 = 500;

/// Построить Settings-страницу. Корневой widget — `adw::PreferencesPage`
/// (имеет встроенный скролл и отступы).
pub fn build(settings: SharedSettings) -> gtk::Widget {
    let page = adw::PreferencesPage::new();

    let save_pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

    // Общие настройки: папка, контейнер, источник.
    page.add(&build_general_group(&settings, &save_pending));

    // Настройки записи (кодек, FPS, битрейт, энкодер, курсор).
    page.add(&build_recording_group(&settings, &save_pending));

    // Аудио.
    page.add(&build_audio_group(&settings, &save_pending));

    // Распознавание речи (OpenAI).
    page.add(&build_stt_api_group(&settings, &save_pending));
    page.add(&build_stt_model_group(&settings, &save_pending));
    page.add(&build_stt_lang_group(&settings, &save_pending));

    // Горячие клавиши.
    page.add(&build_hotkeys_group(&settings, &save_pending));

    page.upcast()
}

fn schedule_save(settings: &SharedSettings, save_pending: &Rc<RefCell<Option<glib::SourceId>>>) {
    if let Some(old) = save_pending.borrow_mut().take() {
        old.remove();
    }
    let settings = settings.clone();
    let save_pending_for_closure = save_pending.clone();
    let source = glib::timeout_add_local(
        std::time::Duration::from_millis(SAVE_DEBOUNCE_MS),
        move || {
            let snapshot = { settings.read().unwrap().clone() };
            if let Err(e) = crate::config::save(&snapshot) {
                tracing::warn!(%e, "failed to save settings");
            } else {
                tracing::debug!("settings saved");
            }
            *save_pending_for_closure.borrow_mut() = None;
            glib::Continue(false)
        },
    );
    *save_pending.borrow_mut() = Some(source);
}

// -------------------- Группы --------------------

fn build_general_group(
    settings: &SharedSettings,
    save_pending: &Rc<RefCell<Option<glib::SourceId>>>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Общие").build();

    let out_row = adw::ActionRow::builder()
        .title("Папка для записей")
        .subtitle(settings.read().unwrap().output_dir.display().to_string())
        .build();
    let pick_btn = gtk::Button::builder()
        .valign(gtk::Align::Center)
        .icon_name("document-open-symbolic")
        .tooltip_text("Выбрать папку")
        .build();
    pick_btn.add_css_class("flat");

    {
        let settings = settings.clone();
        let save_pending = save_pending.clone();
        let row = out_row.clone();
        let group_weak = group.downgrade();
        pick_btn.connect_clicked(move |_btn| {
            let dialog = gtk::FileChooserNative::new(
                Some("Выбрать папку для записей"),
                group_weak
                    .upgrade()
                    .and_then(|g| g.root())
                    .and_then(|r| r.downcast::<gtk::Window>().ok())
                    .as_ref(),
                gtk::FileChooserAction::SelectFolder,
                Some("Выбрать"),
                Some("Отмена"),
            );
            let settings = settings.clone();
            let save_pending = save_pending.clone();
            let row = row.clone();
            dialog.connect_response(move |d, resp| {
                if resp == gtk::ResponseType::Accept {
                    if let Some(file) = d.file() {
                        if let Some(path) = file.path() {
                            settings.write().unwrap().output_dir = path.clone();
                            row.set_subtitle(&path.display().to_string());
                            schedule_save(&settings, &save_pending);
                        }
                    }
                }
            });
            dialog.show();
        });
    }
    out_row.add_suffix(&pick_btn);
    group.add(&out_row);

    // Capture source (Screen / Window) — зеркалит card-кнопки на Record-экране.
    let source_row = make_combo_row(
        "Источник по умолчанию",
        &["Весь экран", "Окно"],
        capture_source_to_index(settings.read().unwrap().capture_source),
    );
    {
        let settings = settings.clone();
        let save_pending = save_pending.clone();
        source_row.connect_selected_notify(move |row| {
            settings.write().unwrap().capture_source = capture_source_from_index(row.selected());
            schedule_save(&settings, &save_pending);
        });
    }
    group.add(&source_row);

    let cursor_row = make_combo_row(
        "Курсор",
        &["Скрыт", "Впечатан в видео", "Отдельная дорожка"],
        cursor_to_index(settings.read().unwrap().cursor_mode),
    );
    {
        let settings = settings.clone();
        let save_pending = save_pending.clone();
        cursor_row.connect_selected_notify(move |row| {
            settings.write().unwrap().cursor_mode = cursor_from_index(row.selected());
            schedule_save(&settings, &save_pending);
        });
    }
    group.add(&cursor_row);

    group
}

fn build_recording_group(
    settings: &SharedSettings,
    save_pending: &Rc<RefCell<Option<glib::SourceId>>>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Запись")
        .description("Параметры применяются к следующей записи.")
        .build();

    // Countdown (phase 19.a.7) — сегментированный 0/3/5/10 секунд.
    group.add(&build_countdown_row(settings, save_pending));

    let container_row = make_combo_row(
        "Контейнер",
        &["MKV (рекомендуется)", "MP4", "WebM"],
        container_to_index(settings.read().unwrap().container),
    );
    {
        let settings = settings.clone();
        let save_pending = save_pending.clone();
        container_row.connect_selected_notify(move |row| {
            settings.write().unwrap().container = container_from_index(row.selected());
            schedule_save(&settings, &save_pending);
        });
    }
    group.add(&container_row);

    let codec_row = make_combo_row(
        "Кодек",
        &["H.264 (активен)", "H.265 (TBD)", "VP9 (TBD)", "AV1 (TBD)"],
        codec_to_index(settings.read().unwrap().video_codec),
    );
    {
        let settings = settings.clone();
        let save_pending = save_pending.clone();
        codec_row.connect_selected_notify(move |row| {
            settings.write().unwrap().video_codec = codec_from_index(row.selected());
            schedule_save(&settings, &save_pending);
        });
    }
    group.add(&codec_row);

    group.add(&make_spin_row(
        "Частота кадров (fps)",
        "От 5 до 60 — выше частота, больше файл.",
        5,
        60,
        1,
        settings.read().unwrap().fps as i32,
        |val, s| s.fps = val as u32,
        settings,
        save_pending,
    ));

    group.add(&make_spin_row(
        "Видео-битрейт (kbps)",
        "Выше битрейт — выше качество и размер.",
        500,
        20000,
        500,
        settings.read().unwrap().video_bitrate as i32,
        |val, s| s.video_bitrate = val as u32,
        settings,
        save_pending,
    ));

    let hint_row = make_combo_row(
        "Энкодер",
        &[
            "Авто (HW → SW fallback)",
            "Только HW (VAAPI / NVENC / QSV)",
            "Только SW (x264enc)",
        ],
        encoder_hint_to_index(settings.read().unwrap().encoder_hint),
    );
    {
        let settings = settings.clone();
        let save_pending = save_pending.clone();
        hint_row.connect_selected_notify(move |row| {
            settings.write().unwrap().encoder_hint = encoder_hint_from_index(row.selected());
            schedule_save(&settings, &save_pending);
        });
    }
    group.add(&hint_row);

    let detect_row = adw::ActionRow::builder()
        .title("Проверить доступные энкодеры")
        .subtitle("Список factory-имён, обнаруженных GStreamer")
        .build();
    let detect_btn = gtk::Button::builder()
        .label("Показать")
        .valign(gtk::Align::Center)
        .build();
    detect_btn.add_css_class("flat");
    let group_weak = group.downgrade();
    detect_btn.connect_clicked(move |_| {
        let encoders = detect_available_encoders();
        let body = if encoders.is_empty() {
            "Видео-энкодеры не обнаружены. Проверь gstreamer1.0-plugins-ugly / libva.".to_owned()
        } else {
            encoders
                .iter()
                .map(|e| format!("• {}  —  {}", e.factory_name, e.backend.label()))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let parent_win = group_weak
            .upgrade()
            .and_then(|g| g.root())
            .and_then(|r| r.downcast::<gtk::Window>().ok());
        let dialog = gtk::MessageDialog::builder()
            .modal(true)
            .message_type(gtk::MessageType::Info)
            .buttons(gtk::ButtonsType::Close)
            .text("Доступные энкодеры")
            .secondary_text(body)
            .build();
        if let Some(w) = parent_win.as_ref() {
            dialog.set_transient_for(Some(w));
        }
        dialog.connect_response(|d, _| d.close());
        dialog.present();
    });
    detect_row.add_suffix(&detect_btn);
    group.add(&detect_row);

    group
}

fn build_audio_group(
    settings: &SharedSettings,
    save_pending: &Rc<RefCell<Option<glib::SourceId>>>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Аудио").build();

    let mode_row = make_combo_row(
        "Режим",
        &[
            "Отдельные дорожки",
            "Микс в одну дорожку (рекомендуется для live)",
        ],
        audio_mode_to_index(settings.read().unwrap().audio_mode),
    );
    {
        let settings = settings.clone();
        let save_pending = save_pending.clone();
        mode_row.connect_selected_notify(move |row| {
            settings.write().unwrap().audio_mode = audio_mode_from_index(row.selected());
            schedule_save(&settings, &save_pending);
        });
    }
    group.add(&mode_row);

    group.add(&make_spin_row(
        "Аудио-битрейт (kbps)",
        "Для голоса 64–96, для музыки 128–192.",
        32,
        320,
        16,
        settings.read().unwrap().audio_bitrate as i32,
        |val, s| s.audio_bitrate = val as u32,
        settings,
        save_pending,
    ));

    group
}

fn build_stt_api_group(
    settings: &SharedSettings,
    save_pending: &Rc<RefCell<Option<glib::SourceId>>>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("OpenAI API")
        .description("Ключ хранится в ~/.config/ralume/settings.toml (chmod 600).")
        .build();

    let key_row = adw::ActionRow::builder().title("API-ключ").build();
    let entry = gtk::Entry::builder()
        .valign(gtk::Align::Center)
        .input_purpose(gtk::InputPurpose::Password)
        .visibility(false)
        .text(settings.read().unwrap().openai_api_key.as_str())
        .placeholder_text("sk-…")
        .width_chars(24)
        .build();
    {
        let settings = settings.clone();
        let save_pending = save_pending.clone();
        entry.connect_changed(move |e| {
            settings.write().unwrap().openai_api_key = e.text().to_string();
            schedule_save(&settings, &save_pending);
        });
    }
    let reveal_btn = gtk::Button::builder()
        .valign(gtk::Align::Center)
        .icon_name("view-reveal-symbolic")
        .tooltip_text("Показать/скрыть ключ")
        .build();
    reveal_btn.add_css_class("flat");
    {
        let entry = entry.clone();
        let shown = std::cell::Cell::new(false);
        reveal_btn.connect_clicked(move |btn| {
            let v = !shown.get();
            shown.set(v);
            entry.set_visibility(v);
            btn.set_icon_name(if v {
                "view-conceal-symbolic"
            } else {
                "view-reveal-symbolic"
            });
        });
    }
    key_row.add_suffix(&entry);
    key_row.add_suffix(&reveal_btn);
    key_row.set_activatable_widget(Some(&entry));
    group.add(&key_row);
    group
}

fn build_countdown_row(
    settings: &SharedSettings,
    save_pending: &Rc<RefCell<Option<glib::SourceId>>>,
) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title("Отсчёт перед стартом")
        .subtitle("Задержка в секундах — успеть переключиться на нужное окно.")
        .build();

    let current = settings.read().unwrap().countdown_seconds;

    let segmented = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .valign(gtk::Align::Center)
        .build();
    segmented.add_css_class("linked");

    let mut buttons: Vec<(gtk::ToggleButton, u32)> = Vec::new();
    let choices: &[(u32, &str)] = &[(0, "Off"), (3, "3 с"), (5, "5 с"), (10, "10 с")];
    let mut group_anchor: Option<gtk::ToggleButton> = None;
    for (value, label) in choices {
        let btn = gtk::ToggleButton::builder()
            .label(*label)
            .active(*value == current)
            .valign(gtk::Align::Center)
            .build();
        if let Some(anchor) = group_anchor.as_ref() {
            btn.set_group(Some(anchor));
        } else {
            group_anchor = Some(btn.clone());
        }
        segmented.append(&btn);
        buttons.push((btn, *value));
    }
    for (btn, value) in &buttons {
        let settings = settings.clone();
        let save_pending = save_pending.clone();
        let v = *value;
        btn.connect_toggled(move |b| {
            if !b.is_active() {
                return;
            }
            settings.write().unwrap().countdown_seconds = v;
            schedule_save(&settings, &save_pending);
        });
    }

    row.add_suffix(&segmented);
    row
}

fn build_stt_model_group(
    settings: &SharedSettings,
    save_pending: &Rc<RefCell<Option<glib::SourceId>>>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Модель").build();
    let model_row = make_combo_row(
        "Модель распознавания",
        &[
            "GPT-4o Mini Transcribe ($0.003/мин, рекомендуется)",
            "GPT-4o Transcribe ($0.006/мин, качество)",
            "Whisper-1 ($0.006/мин, поддержка SRT)",
            "GPT-4o Transcribe + Diarize ($0.006/мин, дикторы)",
        ],
        model_to_index(settings.read().unwrap().transcription_model),
    );
    let current = settings.read().unwrap().transcription_model;
    let desc_row = adw::ActionRow::builder()
        .title(current.label())
        .subtitle(model_description(current))
        .build();
    {
        let settings = settings.clone();
        let save_pending = save_pending.clone();
        let desc_row = desc_row.clone();
        model_row.connect_selected_notify(move |row| {
            let m = model_from_index(row.selected());
            settings.write().unwrap().transcription_model = m;
            desc_row.set_title(m.label());
            desc_row.set_subtitle(model_description(m));
            schedule_save(&settings, &save_pending);
        });
    }
    group.add(&model_row);
    group.add(&desc_row);
    group
}

fn build_stt_lang_group(
    settings: &SharedSettings,
    save_pending: &Rc<RefCell<Option<glib::SourceId>>>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Язык и лимиты")
        .description("Оставьте язык пустым для авто-определения. Примеры: ru, en.")
        .build();
    let lang_row = adw::ActionRow::builder().title("Язык").build();
    let lang_entry = gtk::Entry::builder()
        .valign(gtk::Align::Center)
        .text(settings.read().unwrap().transcription_language.as_str())
        .placeholder_text("auto")
        .width_chars(6)
        .build();
    {
        let settings = settings.clone();
        let save_pending = save_pending.clone();
        lang_entry.connect_changed(move |e| {
            let txt = e.text().to_string();
            let valid =
                txt.is_empty() || (txt.len() == 2 && txt.chars().all(|c| c.is_ascii_alphabetic()));
            if valid {
                e.remove_css_class("error");
                settings.write().unwrap().transcription_language = txt.to_lowercase();
                schedule_save(&settings, &save_pending);
            } else {
                e.add_css_class("error");
            }
        });
    }
    lang_row.add_suffix(&lang_entry);
    lang_row.set_activatable_widget(Some(&lang_entry));
    group.add(&lang_row);

    let help_row = adw::ActionRow::builder()
        .title("Лимит файла — 25 МБ")
        .subtitle("Длинные записи делятся на части и склеиваются в один .txt")
        .build();
    let docs_btn = gtk::Button::builder()
        .label("Документация")
        .valign(gtk::Align::Center)
        .build();
    docs_btn.add_css_class("flat");
    docs_btn.connect_clicked(|_| {
        if let Err(e) = gio::AppInfo::launch_default_for_uri(
            "https://platform.openai.com/docs/guides/speech-to-text",
            gio::AppLaunchContext::NONE,
        ) {
            tracing::warn!(%e, "failed to open stt docs");
        }
    });
    help_row.add_suffix(&docs_btn);
    group.add(&help_row);

    group
}

fn build_hotkeys_group(
    settings: &SharedSettings,
    save_pending: &Rc<RefCell<Option<glib::SourceId>>>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Горячие клавиши")
        .description("Будут зарегистрированы через xdg-desktop-portal в Phase 9.1.")
        .build();

    let hotkey_row = adw::ActionRow::builder()
        .title("Start / Stop")
        .subtitle("Формат GTK-акселератора, например <Ctrl><Alt>R")
        .build();
    let entry = gtk::Entry::builder()
        .valign(gtk::Align::Center)
        .text(settings.read().unwrap().hotkey_start_stop.as_str())
        .width_chars(16)
        .build();
    {
        let settings = settings.clone();
        let save_pending = save_pending.clone();
        entry.connect_changed(move |e| {
            let text = e.text().to_string();
            let ok = gtk::accelerator_parse(&text).is_some();
            if ok || text.is_empty() {
                e.remove_css_class("error");
                settings.write().unwrap().hotkey_start_stop = text;
                schedule_save(&settings, &save_pending);
            } else {
                e.add_css_class("error");
            }
        });
    }
    hotkey_row.add_suffix(&entry);
    hotkey_row.set_activatable_widget(Some(&entry));
    group.add(&hotkey_row);

    // Остальные хоткеи — только отображение (регистрация в phase 9.1).
    let display_only: &[(&str, &[&str])] = &[
        ("Пауза / продолжить", &["Ctrl", "Shift", "P"]),
        ("Открыть библиотеку", &["Ctrl", "L"]),
    ];
    for (label, keys) in display_only {
        let row = adw::ActionRow::builder().title(*label).build();
        let hbox = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .valign(gtk::Align::Center)
            .build();
        for (i, k) in keys.iter().enumerate() {
            let chip = gtk::Label::new(Some(k));
            chip.add_css_class("kbd");
            hbox.append(&chip);
            if i + 1 < keys.len() {
                let plus = gtk::Label::new(Some("+"));
                plus.add_css_class("dim-label");
                hbox.append(&plus);
            }
        }
        row.add_suffix(&hbox);
        group.add(&row);
    }

    group
}

// -------------------- helpers --------------------

fn make_combo_row(title: &str, options: &[&str], selected: u32) -> adw::ComboRow {
    let model = gtk::StringList::new(options);
    adw::ComboRow::builder()
        .title(title)
        .model(&model)
        .selected(selected)
        .build()
}

#[allow(clippy::too_many_arguments)]
fn make_spin_row(
    title: &str,
    subtitle: &str,
    min: i32,
    max: i32,
    step: i32,
    current: i32,
    apply: impl Fn(i32, &mut Settings) + 'static,
    settings: &SharedSettings,
    save_pending: &Rc<RefCell<Option<glib::SourceId>>>,
) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build();
    let spin = gtk::SpinButton::with_range(min as f64, max as f64, step as f64);
    spin.set_valign(gtk::Align::Center);
    spin.set_value(current as f64);
    spin.set_numeric(true);
    {
        let settings = settings.clone();
        let save_pending = save_pending.clone();
        spin.connect_value_changed(move |s| {
            let val = s.value() as i32;
            apply(val, &mut settings.write().unwrap());
            schedule_save(&settings, &save_pending);
        });
    }
    row.add_suffix(&spin);
    row.set_activatable_widget(Some(&spin));
    row
}

fn model_description(m: TranscriptionModel) -> &'static str {
    match m {
        TranscriptionModel::Whisper1 => {
            "Классика: таймкоды и SRT/VTT, качество ниже чем 4o на шумных записях."
        }
        TranscriptionModel::Gpt4oTranscribe => {
            "Высшее качество на русском и английском; response_format только text/json."
        }
        TranscriptionModel::Gpt4oMiniTranscribe => {
            "Вдвое дешевле, качество близко к gpt-4o-transcribe; хороший дефолт."
        }
        TranscriptionModel::Gpt4oTranscribeDiarize => {
            "Делит дорожку по дикторам — полезно для созвонов."
        }
    }
}

// -------- enum <-> index --------

fn container_to_index(c: Container) -> u32 {
    match c {
        Container::Mkv => 0,
        Container::Mp4 => 1,
        Container::Webm => 2,
    }
}
fn container_from_index(i: u32) -> Container {
    match i {
        1 => Container::Mp4,
        2 => Container::Webm,
        _ => Container::Mkv,
    }
}

fn codec_to_index(c: VideoCodec) -> u32 {
    match c {
        VideoCodec::H264 => 0,
        VideoCodec::H265 => 1,
        VideoCodec::Vp9 => 2,
        VideoCodec::Av1 => 3,
    }
}
fn codec_from_index(i: u32) -> VideoCodec {
    match i {
        1 => VideoCodec::H265,
        2 => VideoCodec::Vp9,
        3 => VideoCodec::Av1,
        _ => VideoCodec::H264,
    }
}

fn audio_mode_to_index(m: AudioMode) -> u32 {
    match m {
        AudioMode::Separate => 0,
        AudioMode::Mixed => 1,
    }
}
fn audio_mode_from_index(i: u32) -> AudioMode {
    match i {
        1 => AudioMode::Mixed,
        _ => AudioMode::Separate,
    }
}

fn cursor_to_index(m: CursorMode) -> u32 {
    match m {
        CursorMode::Hidden => 0,
        CursorMode::Embedded => 1,
        CursorMode::Metadata => 2,
    }
}
fn cursor_from_index(i: u32) -> CursorMode {
    match i {
        0 => CursorMode::Hidden,
        2 => CursorMode::Metadata,
        _ => CursorMode::Embedded,
    }
}

fn encoder_hint_to_index(h: EncoderHint) -> u32 {
    match h {
        EncoderHint::Auto => 0,
        EncoderHint::Hardware => 1,
        EncoderHint::Software => 2,
    }
}
fn encoder_hint_from_index(i: u32) -> EncoderHint {
    match i {
        1 => EncoderHint::Hardware,
        2 => EncoderHint::Software,
        _ => EncoderHint::Auto,
    }
}

fn model_to_index(m: TranscriptionModel) -> u32 {
    match m {
        TranscriptionModel::Gpt4oMiniTranscribe => 0,
        TranscriptionModel::Gpt4oTranscribe => 1,
        TranscriptionModel::Whisper1 => 2,
        TranscriptionModel::Gpt4oTranscribeDiarize => 3,
    }
}
fn model_from_index(i: u32) -> TranscriptionModel {
    match i {
        1 => TranscriptionModel::Gpt4oTranscribe,
        2 => TranscriptionModel::Whisper1,
        3 => TranscriptionModel::Gpt4oTranscribeDiarize,
        _ => TranscriptionModel::Gpt4oMiniTranscribe,
    }
}

fn capture_source_to_index(c: CaptureSource) -> u32 {
    match c {
        CaptureSource::Screen => 0,
        CaptureSource::Window => 1,
    }
}
fn capture_source_from_index(i: u32) -> CaptureSource {
    match i {
        1 => CaptureSource::Window,
        _ => CaptureSource::Screen,
    }
}
