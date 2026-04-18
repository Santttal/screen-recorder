use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use libadwaita as adw;
use gtk4 as gtk;

use crate::config::{
    AudioMode, Container, CursorMode, EncoderHint, RegionMode, Settings, SharedSettings,
    VideoCodec,
};
use crate::recorder::encoders::detect_available_encoders;

const SAVE_DEBOUNCE_MS: u64 = 500;

#[allow(dead_code)]
pub struct PreferencesWindow {
    window: adw::PreferencesWindow,
    // settings/save_pending удерживаются Rc-ами внутри замыканий, держим ref для симметрии.
    settings: SharedSettings,
    save_pending: Rc<RefCell<Option<glib::SourceId>>>,
}

impl PreferencesWindow {
    pub fn present(parent: &adw::ApplicationWindow, settings: SharedSettings) {
        let this = Rc::new(Self::build(parent, settings));
        this.window.present();
    }

    fn build(parent: &adw::ApplicationWindow, settings: SharedSettings) -> Self {
        let window = adw::PreferencesWindow::builder()
            .transient_for(parent)
            .modal(true)
            .search_enabled(false)
            .build();

        let save_pending = Rc::new(RefCell::new(None));

        window.add(&build_recording_page(&settings, &save_pending));
        window.add(&build_audio_page(&settings, &save_pending));
        window.add(&build_hotkeys_page(&settings, &save_pending));

        Self {
            window,
            settings,
            save_pending,
        }
    }
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

fn build_recording_page(
    settings: &SharedSettings,
    save_pending: &Rc<RefCell<Option<glib::SourceId>>>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Запись")
        .icon_name("video-x-generic-symbolic")
        .build();

    // ── Группа «Файл»
    let out_group = adw::PreferencesGroup::builder()
        .title("Файл")
        .build();

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
        let page_weak = page.downgrade();
        pick_btn.connect_clicked(move |_btn| {
            let dialog = gtk::FileChooserNative::new(
                Some("Выбрать папку для записей"),
                page_weak
                    .upgrade()
                    .and_then(|p| p.root())
                    .and_then(|r| r.downcast::<gtk::Window>().ok())
                    .as_ref(),
                gtk::FileChooserAction::SelectFolder,
                Some("Выбрать"),
                Some("Отмена"),
            );
            if let Ok(cur) =
                gio::File::for_path(&settings.read().unwrap().output_dir).query_info(
                    "standard::type",
                    gio::FileQueryInfoFlags::NONE,
                    gio::Cancellable::NONE,
                )
            {
                let _ = cur; // preserved for future: restore last dir
            }
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
    out_group.add(&out_row);

    // Container
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
    out_group.add(&container_row);
    page.add(&out_group);

    // ── Группа «Видео»
    let video_group = adw::PreferencesGroup::builder().title("Видео").build();

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
    video_group.add(&codec_row);

    video_group.add(&make_spin_row(
        "FPS",
        "Кадров в секунду (5 достаточно для статичного экрана)",
        5,
        60,
        1,
        settings.read().unwrap().fps as i32,
        |val, s| s.fps = val as u32,
        settings,
        save_pending,
    ));

    video_group.add(&make_spin_row(
        "Видео-битрейт (kbps)",
        "Выше битрейт — выше качество и размер",
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
        &["Авто (HW → SW fallback)", "Только HW (VAAPI/NVENC/QSV)", "Только SW (x264enc)"],
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
    video_group.add(&hint_row);

    let detect_row = adw::ActionRow::builder()
        .title("Проверить доступные энкодеры")
        .subtitle("Откроет список factory-имён, обнаруженных GStreamer")
        .build();
    let detect_btn = gtk::Button::builder()
        .label("Показать")
        .valign(gtk::Align::Center)
        .build();
    detect_btn.add_css_class("flat");
    let page_weak = page.downgrade();
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
        let parent_win = page_weak
            .upgrade()
            .and_then(|p| p.root())
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
    video_group.add(&detect_row);

    page.add(&video_group);

    // ── Группа «Источник»
    let src_group = adw::PreferencesGroup::builder().title("Источник").build();

    let region_row = make_combo_row(
        "Область",
        &["Весь экран", "Монитор", "Окно"],
        region_to_index(settings.read().unwrap().region_mode),
    );
    {
        let settings = settings.clone();
        let save_pending = save_pending.clone();
        region_row.connect_selected_notify(move |row| {
            settings.write().unwrap().region_mode = region_from_index(row.selected());
            schedule_save(&settings, &save_pending);
        });
    }
    src_group.add(&region_row);

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
    src_group.add(&cursor_row);
    page.add(&src_group);

    page
}

fn build_audio_page(
    settings: &SharedSettings,
    save_pending: &Rc<RefCell<Option<glib::SourceId>>>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Аудио")
        .icon_name("audio-x-generic-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder().title("Звук").build();

    let mode_row = make_combo_row(
        "Режим аудио",
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
        "Для голоса хватит 64–96, для музыки 128–192",
        32,
        320,
        16,
        settings.read().unwrap().audio_bitrate as i32,
        |val, s| s.audio_bitrate = val as u32,
        settings,
        save_pending,
    ));

    page.add(&group);
    page
}

fn build_hotkeys_page(
    settings: &SharedSettings,
    save_pending: &Rc<RefCell<Option<glib::SourceId>>>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Горячие клавиши")
        .icon_name("input-keyboard-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title("Глобальные клавиши")
        .description("Будут зарегистрированы через xdg-desktop-portal в Фазе 9.")
        .build();

    let hotkey_row = adw::ActionRow::builder()
        .title("Start / Stop")
        .subtitle("Формат GTK-акселератора, например &lt;Ctrl&gt;&lt;Alt&gt;R")
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

    page.add(&group);
    page
}

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

// ── helpers: enum ↔ index

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

fn region_to_index(m: RegionMode) -> u32 {
    match m {
        RegionMode::FullScreen => 0,
        RegionMode::Monitor => 1,
        RegionMode::Window => 2,
    }
}
fn region_from_index(i: u32) -> RegionMode {
    match i {
        0 => RegionMode::FullScreen,
        2 => RegionMode::Window,
        _ => RegionMode::Monitor,
    }
}
