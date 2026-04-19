//! Экран «Запись». Новый layout по дизайну Ralume AI (phase 19.a.4):
//! заголовок, source-карточки (Весь экран / Окно), Audio group
//! (Микрофон + Звук системы), Options group (segmented Auto / Manual),
//! hint-подсказка, большая кнопка Start + keybind hint.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use libadwaita as adw;
use gtk4 as gtk;

use crate::config::SharedSettings;

/// Выбор источника захвата экрана. Храним как UI-side state в phase 19.a.4.
/// В 19.a.5 переезжает в `Settings.capture_source` и привязывается к portal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureSource {
    Screen,
    Window,
}

/// Виджеты Record-страницы, за которые держится AppShell.
#[allow(dead_code)]
pub struct RecordPage {
    pub root: gtk::Box,
    pub btn_start_stop: gtk::Button,
    pub switch_sys: gtk::Switch,
    pub switch_mic: gtk::Switch,
    pub seg_auto: gtk::ToggleButton,
    pub seg_manual: gtk::ToggleButton,
    pub card_screen: gtk::ToggleButton,
    pub card_window: gtk::ToggleButton,
    pub lbl_timer: gtk::Label,
    pub lbl_status: gtk::Label,
    pub stt_spinner: gtk::Spinner,
    pub capture_source: Rc<RefCell<CaptureSource>>,
    pub row_stt: adw::ActionRow,
}

/// Построить Record-страницу. Биндинги UI→Settings для Auto/Manual и cards
/// настраиваются здесь же; state-machine-биндинги (start/stop, sensitivity)
/// остаются в AppShell.
pub fn build(settings: &SharedSettings) -> RecordPage {
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(32)
        .margin_end(32)
        .build();

    // ----- Заголовок -----
    let title = gtk::Label::builder()
        .label("Новая запись")
        .halign(gtk::Align::Start)
        .build();
    title.add_css_class("title-1");

    let subtitle = gtk::Label::builder()
        .label("Выберите, что захватить, и нажмите «Начать запись».")
        .halign(gtk::Align::Start)
        .build();
    subtitle.add_css_class("dim-label");

    root.append(&title);
    root.append(&subtitle);

    // ----- Source cards -----
    let (source_group, card_screen, card_window, capture_source) = build_source_group();
    root.append(&source_group);

    // ----- Audio group -----
    let (audio_group, switch_sys, switch_mic) = build_audio_group();
    root.append(&audio_group);

    // ----- Options group (Transcription segmented) -----
    let (options_group, seg_auto, seg_manual, row_stt) =
        build_options_group(settings);
    root.append(&options_group);

    // ----- Big Start button + shortcut hint -----
    let btn_start_stop = gtk::Button::builder()
        .halign(gtk::Align::Center)
        .build();
    btn_start_stop.add_css_class("suggested-action");
    btn_start_stop.add_css_class("pill");
    btn_start_stop.set_child(Some(&build_start_content("Начать запись")));

    let shortcut_hint = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .halign(gtk::Align::Center)
        .build();
    for key in ["Ctrl", "Shift", "R"] {
        let kbd = gtk::Label::new(Some(key));
        kbd.add_css_class("kbd");
        shortcut_hint.append(&kbd);
        if key != "R" {
            let plus = gtk::Label::new(Some("+"));
            plus.add_css_class("dim-label");
            shortcut_hint.append(&plus);
        }
    }
    let shortcut_caption = gtk::Label::new(Some("для быстрого старта"));
    shortcut_caption.add_css_class("caption");
    shortcut_caption.add_css_class("dim-label");
    shortcut_hint.append(&shortcut_caption);

    let action_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .halign(gtk::Align::Center)
        .margin_top(8)
        .build();
    action_box.append(&btn_start_stop);
    action_box.append(&shortcut_hint);
    root.append(&action_box);

    // ----- Timer + status (legacy, remains until floating toolbar в 19.c.4) -----
    let lbl_timer = gtk::Label::new(Some("00:00:00"));
    lbl_timer.add_css_class("title-2");
    lbl_timer.add_css_class("timer-label");
    lbl_timer.set_visible(false);
    root.append(&lbl_timer);

    let lbl_status = gtk::Label::new(Some("Готов"));
    lbl_status.add_css_class("dim-label");
    let stt_spinner = gtk::Spinner::new();
    stt_spinner.set_visible(false);
    let status_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::Center)
        .build();
    status_row.append(&stt_spinner);
    status_row.append(&lbl_status);
    root.append(&status_row);

    // ----- Биндинги UI→Settings -----
    bind_segmented(&seg_auto, &seg_manual, &row_stt, settings);

    RecordPage {
        root,
        btn_start_stop,
        switch_sys,
        switch_mic,
        seg_auto,
        seg_manual,
        card_screen,
        card_window,
        lbl_timer,
        lbl_status,
        stt_spinner,
        capture_source,
        row_stt,
    }
}

fn build_source_group() -> (gtk::Box, gtk::ToggleButton, gtk::ToggleButton, Rc<RefCell<CaptureSource>>) {
    let container = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .build();

    let header = gtk::Label::builder()
        .label("Источник захвата")
        .halign(gtk::Align::Start)
        .build();
    header.add_css_class("heading");
    container.append(&header);

    let flow = gtk::FlowBox::builder()
        .column_spacing(12)
        .row_spacing(12)
        .max_children_per_line(2)
        .min_children_per_line(2)
        .homogeneous(true)
        .selection_mode(gtk::SelectionMode::None)
        .build();

    let card_screen = make_source_card(
        "video-display-symbolic",
        "Весь экран",
        "Запись основного дисплея целиком.",
    );
    card_screen.set_active(true);

    let card_window = make_source_card(
        "window-new-symbolic",
        "Окно",
        "Выбор конкретного окна приложения.",
    );
    card_window.set_group(Some(&card_screen));

    flow.append(&card_screen);
    flow.append(&card_window);
    container.append(&flow);

    // Info-block под картами (разрешение / монитор / формат).
    // В 19.a.4 показываем статический текст; живые данные — когда будет доступ
    // к PortalState (уже хранится в recorder-loop, но не читается UI сейчас).
    let info = gtk::Label::builder()
        .label("Нативное разрешение монитора · H.264")
        .halign(gtk::Align::Start)
        .build();
    info.add_css_class("caption");
    info.add_css_class("dim-label");
    info.set_margin_top(4);
    container.append(&info);

    let capture_source = Rc::new(RefCell::new(CaptureSource::Screen));

    {
        let cs = capture_source.clone();
        card_screen.connect_toggled(move |b| {
            if b.is_active() {
                *cs.borrow_mut() = CaptureSource::Screen;
            }
        });
    }
    {
        let cs = capture_source.clone();
        card_window.connect_toggled(move |b| {
            if b.is_active() {
                *cs.borrow_mut() = CaptureSource::Window;
            }
        });
    }

    (container, card_screen, card_window, capture_source)
}

fn make_source_card(icon_name: &str, title: &str, desc: &str) -> gtk::ToggleButton {
    let btn = gtk::ToggleButton::new();
    btn.add_css_class("source-card");

    let vbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();

    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_icon_size(gtk::IconSize::Large);
    icon.set_halign(gtk::Align::Start);
    vbox.append(&icon);

    let title_lbl = gtk::Label::builder()
        .label(title)
        .halign(gtk::Align::Start)
        .build();
    title_lbl.add_css_class("heading");
    vbox.append(&title_lbl);

    let desc_lbl = gtk::Label::builder()
        .label(desc)
        .halign(gtk::Align::Start)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .build();
    desc_lbl.add_css_class("caption");
    desc_lbl.add_css_class("dim-label");
    vbox.append(&desc_lbl);

    btn.set_child(Some(&vbox));
    btn
}

fn build_audio_group() -> (adw::PreferencesGroup, gtk::Switch, gtk::Switch) {
    let group = adw::PreferencesGroup::new();
    group.set_title("Аудио");

    let row_mic = adw::ActionRow::builder().title("Микрофон").build();
    let switch_mic = gtk::Switch::builder()
        .valign(gtk::Align::Center)
        .active(false)
        .build();
    let mic_icon = gtk::Image::from_icon_name("audio-input-microphone-symbolic");
    row_mic.add_prefix(&mic_icon);
    row_mic.add_suffix(&switch_mic);
    row_mic.set_activatable_widget(Some(&switch_mic));

    let row_sys = adw::ActionRow::builder().title("Звук системы").build();
    let switch_sys = gtk::Switch::builder()
        .valign(gtk::Align::Center)
        .active(true)
        .build();
    let sys_icon = gtk::Image::from_icon_name("audio-volume-high-symbolic");
    row_sys.add_prefix(&sys_icon);
    row_sys.add_suffix(&switch_sys);
    row_sys.set_activatable_widget(Some(&switch_sys));

    group.add(&row_mic);
    group.add(&row_sys);

    (group, switch_sys, switch_mic)
}

fn build_options_group(
    settings: &SharedSettings,
) -> (
    gtk::Box,
    gtk::ToggleButton,
    gtk::ToggleButton,
    adw::ActionRow,
) {
    let container = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();

    let group = adw::PreferencesGroup::new();
    group.set_title("Опции");

    let row = adw::ActionRow::builder().title("Распознавание речи").build();
    let icon = gtk::Image::from_icon_name("starred-symbolic");
    row.add_prefix(&icon);

    let auto_enabled = settings.read().unwrap().transcription_enabled;
    let seg_auto = gtk::ToggleButton::builder()
        .label("Auto")
        .active(auto_enabled)
        .valign(gtk::Align::Center)
        .build();
    let seg_manual = gtk::ToggleButton::builder()
        .label("Manual")
        .active(!auto_enabled)
        .valign(gtk::Align::Center)
        .build();
    seg_manual.set_group(Some(&seg_auto));

    let segmented = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .valign(gtk::Align::Center)
        .build();
    segmented.add_css_class("linked");
    segmented.append(&seg_auto);
    segmented.append(&seg_manual);

    row.add_suffix(&segmented);
    update_row_subtitle(&row, auto_enabled);
    group.add(&row);

    container.append(&group);

    let hint = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_top(2)
        .build();
    let hint_icon = gtk::Image::from_icon_name("dialog-information-symbolic");
    hint_icon.set_icon_size(gtk::IconSize::Normal);
    let hint_text = gtk::Label::builder()
        .label("Формат видео, частота кадров и countdown — в Настройках.")
        .halign(gtk::Align::Start)
        .build();
    hint_text.add_css_class("caption");
    hint_text.add_css_class("dim-label");
    hint.append(&hint_icon);
    hint.append(&hint_text);
    container.append(&hint);

    (container, seg_auto, seg_manual, row)
}

fn update_row_subtitle(row: &adw::ActionRow, auto: bool) {
    let subtitle = if auto {
        "Запускается автоматически после сохранения."
    } else {
        "Запускается вручную из Библиотеки."
    };
    row.set_subtitle(subtitle);
}

fn bind_segmented(
    seg_auto: &gtk::ToggleButton,
    seg_manual: &gtk::ToggleButton,
    row: &adw::ActionRow,
    settings: &SharedSettings,
) {
    let settings_c = settings.clone();
    let row_c = row.clone();
    seg_auto.connect_toggled(move |b| {
        if !b.is_active() {
            return;
        }
        update_row_subtitle(&row_c, true);
        settings_c.write().unwrap().transcription_enabled = true;
        let snapshot = settings_c.read().unwrap().clone();
        if let Err(e) = crate::config::save(&snapshot) {
            tracing::warn!(%e, "failed to persist transcription toggle");
        }
    });

    let settings_c = settings.clone();
    let row_c = row.clone();
    seg_manual.connect_toggled(move |b| {
        if !b.is_active() {
            return;
        }
        update_row_subtitle(&row_c, false);
        settings_c.write().unwrap().transcription_enabled = false;
        let snapshot = settings_c.read().unwrap().clone();
        if let Err(e) = crate::config::save(&snapshot) {
            tracing::warn!(%e, "failed to persist transcription toggle");
        }
    });
}

pub fn build_start_content(label: &str) -> gtk::Box {
    let hbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    let dot = gtk::Label::new(Some("●"));
    dot.add_css_class("recording-dot");
    hbox.append(&dot);
    let lbl = gtk::Label::new(Some(label));
    hbox.append(&lbl);
    hbox
}
