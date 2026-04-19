use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use adw::prelude::*;
use ashpd::WindowIdentifier;
use async_channel::Sender;
use gstreamer as gst;
use gstreamer::prelude::*;
use gtk::gio;
use gtk::glib;
use libadwaita as adw;
use gtk4 as gtk;

use crate::config::SharedSettings;
use crate::recorder::{
    attach_bus_watch, build_pipeline, start as pipeline_start, stop_graceful, RecordRequest,
};
use crate::ui::events::{RecorderEvent, UiCommand};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct Sources {
    pub screen: bool,
    pub system_audio: bool,
    pub microphone: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiRecordingState {
    Idle,
    Preparing,
    Recording,
    Finalizing,
}

impl UiRecordingState {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Preparing | Self::Recording | Self::Finalizing)
    }
}

pub struct AppShell {
    window: adw::ApplicationWindow,
    btn_start_stop: gtk::Button,
    lbl_status: gtk::Label,
    lbl_timer: gtk::Label,
    switch_screen: gtk::Switch,
    switch_sys: gtk::Switch,
    switch_mic: gtk::Switch,
    switch_stt: gtk::Switch,
    stt_spinner: gtk::Spinner,
    lbl_rec_dot: gtk::Label,
    toast_overlay: adw::ToastOverlay,
    state: Rc<RefCell<UiRecordingState>>,
    timer_source: Rc<RefCell<Option<glib::SourceId>>>,
    pipeline: Rc<RefCell<Option<gst::Pipeline>>>,
    bus_guard: Rc<RefCell<Option<glib::SourceId>>>,
    force_null_watchdog: Rc<RefCell<Option<glib::SourceId>>>,
    cmd_tx: Sender<UiCommand>,
    evt_tx: Sender<RecorderEvent>,
    settings: SharedSettings,
}

impl AppShell {
    pub fn new(
        app: &adw::Application,
        cmd_tx: Sender<UiCommand>,
        evt_tx: Sender<RecorderEvent>,
        settings: SharedSettings,
    ) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Ralume")
            .default_width(960)
            .default_height(640)
            .build();

        let header = adw::HeaderBar::new();

        let lbl_rec_dot = gtk::Label::new(Some("●"));
        lbl_rec_dot.add_css_class("recording-dot");
        lbl_rec_dot.set_visible(false);
        lbl_rec_dot.set_tooltip_text(Some("Идёт запись"));
        header.pack_start(&lbl_rec_dot);

        let menu = gio::Menu::new();
        menu.append(Some("Настройки"), Some("app.preferences"));
        menu.append(Some("О программе"), Some("app.about"));
        menu.append(Some("Выход"), Some("app.quit"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .build();
        header.pack_end(&menu_button);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();

        let group = adw::PreferencesGroup::new();

        let row_screen = adw::ActionRow::builder().title("Экран").build();
        let switch_screen = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .active(true)
            .build();
        row_screen.add_suffix(&switch_screen);
        row_screen.set_activatable_widget(Some(&switch_screen));

        let row_sys = adw::ActionRow::builder().title("Звук системы").build();
        let switch_sys = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .active(true)
            .build();
        row_sys.add_suffix(&switch_sys);
        row_sys.set_activatable_widget(Some(&switch_sys));

        let row_mic = adw::ActionRow::builder().title("Микрофон").build();
        let switch_mic = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .active(false)
            .build();
        row_mic.add_suffix(&switch_mic);
        row_mic.set_activatable_widget(Some(&switch_mic));

        let row_stt = adw::ActionRow::builder()
            .title("Распознавание речи")
            .subtitle("Сохраняет расшифровку в .txt рядом с видео (OpenAI)")
            .build();
        let switch_stt = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .active(settings.read().unwrap().transcription_enabled)
            .build();
        row_stt.add_suffix(&switch_stt);
        row_stt.set_activatable_widget(Some(&switch_stt));

        group.add(&row_screen);
        group.add(&row_sys);
        group.add(&row_mic);
        group.add(&row_stt);
        root.append(&group);

        let btn_start_stop = gtk::Button::builder()
            .label("Начать запись")
            .halign(gtk::Align::Center)
            .build();
        btn_start_stop.add_css_class("suggested-action");
        btn_start_stop.add_css_class("pill");
        root.append(&btn_start_stop);

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

        // Sidebar + content Stack layout (phase 19.a.3).
        // The existing Record-page widgets go into stack child "record".
        // Library / AI / Settings are placeholders for phases 19.a.6 / 19.b / 19.c.
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(150)
            .hexpand(true)
            .vexpand(true)
            .build();
        stack.add_named(&root, Some("record"));
        stack.add_named(&build_placeholder_page("Library", "Откроется в фазе 19.b."), Some("library"));
        stack.add_named(&build_placeholder_page("AI", "Откроется в фазе 19.c."), Some("ai"));
        stack.add_named(&build_placeholder_page("Settings", "Переедет из отдельного окна в фазе 19.a.6."), Some("settings"));

        let sidebar = build_sidebar(&stack);

        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();
        body.append(&sidebar);
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        body.append(&stack);

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&body));

        let main_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        main_box.append(&header);
        main_box.append(&toast_overlay);
        window.set_content(Some(&main_box));

        let this = Rc::new(Self {
            window,
            btn_start_stop: btn_start_stop.clone(),
            lbl_status,
            lbl_timer,
            switch_screen,
            switch_sys,
            switch_mic,
            switch_stt: switch_stt.clone(),
            stt_spinner,
            lbl_rec_dot,
            toast_overlay,
            state: Rc::new(RefCell::new(UiRecordingState::Idle)),
            timer_source: Rc::new(RefCell::new(None)),
            pipeline: Rc::new(RefCell::new(None)),
            bus_guard: Rc::new(RefCell::new(None)),
            force_null_watchdog: Rc::new(RefCell::new(None)),
            cmd_tx,
            evt_tx,
            settings,
        });

        {
            let settings = this.settings.clone();
            let weak_self = Rc::downgrade(&this);
            switch_stt.connect_state_set(move |_w, new_state| {
                settings.write().unwrap().transcription_enabled = new_state;
                let snapshot = settings.read().unwrap().clone();
                if let Err(e) = crate::config::save(&snapshot) {
                    tracing::warn!(%e, "failed to persist transcription toggle");
                }
                if new_state && snapshot.openai_api_key.trim().is_empty() {
                    if let Some(me) = weak_self.upgrade() {
                        me.show_toast("Укажите API-ключ OpenAI в Настройках");
                    }
                }
                glib::signal::Inhibit(false)
            });
        }

        let weak_self = Rc::downgrade(&this);
        btn_start_stop.connect_clicked(move |_| {
            let weak = weak_self.clone();
            glib::MainContext::default().spawn_local(async move {
                if let Some(me) = weak.upgrade() {
                    me.on_start_stop_clicked().await;
                }
            });
        });

        let weak_close = Rc::downgrade(&this);
        this.window.connect_close_request(move |_| {
            if let Some(me) = weak_close.upgrade() {
                if me.state().is_active() {
                    me.prompt_close_confirmation();
                    return glib::signal::Inhibit(true);
                }
            }
            glib::signal::Inhibit(false)
        });

        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn window(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    pub fn sources_snapshot(&self) -> Sources {
        Sources {
            screen: self.switch_screen.is_active(),
            system_audio: self.switch_sys.is_active(),
            microphone: self.switch_mic.is_active(),
        }
    }

    pub fn state(&self) -> UiRecordingState {
        *self.state.borrow()
    }

    pub fn set_recording_state(&self, state: UiRecordingState) {
        *self.state.borrow_mut() = state;
        self.lbl_rec_dot
            .set_visible(matches!(state, UiRecordingState::Recording));
        match state {
            UiRecordingState::Idle => {
                self.btn_start_stop.set_label("Начать запись");
                self.btn_start_stop.set_sensitive(true);
                self.btn_start_stop.remove_css_class("destructive-action");
                self.btn_start_stop.add_css_class("suggested-action");
                self.set_sources_sensitive(true);
            }
            UiRecordingState::Preparing => {
                self.btn_start_stop.set_label("Подготовка…");
                self.btn_start_stop.set_sensitive(false);
                self.set_sources_sensitive(false);
            }
            UiRecordingState::Recording => {
                self.btn_start_stop.set_label("Остановить");
                self.btn_start_stop.set_sensitive(true);
                self.btn_start_stop.remove_css_class("suggested-action");
                self.btn_start_stop.add_css_class("destructive-action");
                self.set_sources_sensitive(false);
            }
            UiRecordingState::Finalizing => {
                self.btn_start_stop.set_label("Сохранение…");
                self.btn_start_stop.set_sensitive(false);
                self.set_sources_sensitive(false);
            }
        }
    }

    fn set_sources_sensitive(&self, sensitive: bool) {
        self.switch_screen.set_sensitive(sensitive);
        self.switch_sys.set_sensitive(sensitive);
        self.switch_mic.set_sensitive(sensitive);
        self.switch_stt.set_sensitive(sensitive);
    }

    pub fn transcription_enabled(&self) -> bool {
        self.switch_stt.is_active()
    }

    pub fn set_status(&self, text: &str) {
        self.lbl_status.set_label(text);
    }

    pub fn set_stt_busy(&self, busy: bool) {
        self.stt_spinner.set_visible(busy);
        if busy {
            self.stt_spinner.start();
        } else {
            self.stt_spinner.stop();
        }
    }

    pub fn show_saved_toast(&self, path: &std::path::Path) {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let toast = adw::Toast::builder()
            .title(&format!("Сохранено: {name}"))
            .button_label("Показать")
            .action_name("app.show-file")
            .timeout(3)
            .build();
        if let Some(d) = path.parent() {
            toast.set_action_target_value(Some(&d.to_string_lossy().to_string().to_variant()));
        }
        self.toast_overlay.add_toast(toast);
    }

    pub fn show_saved_text_toast(&self, path: &std::path::Path) {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let toast = adw::Toast::builder()
            .title(&format!("Расшифровка сохранена: {name}"))
            .button_label("Показать")
            .action_name("app.show-file")
            .timeout(3)
            .build();
        if let Some(d) = path.parent() {
            toast.set_action_target_value(Some(&d.to_string_lossy().to_string().to_variant()));
        }
        self.toast_overlay.add_toast(toast);
    }

    pub fn show_toast(&self, text: &str) {
        let trimmed = if text.chars().count() > 120 {
            let cut: String = text.chars().take(117).collect();
            format!("{cut}…")
        } else {
            text.to_owned()
        };
        let toast = adw::Toast::builder().title(&trimmed).timeout(3).build();
        self.toast_overlay.add_toast(toast);
    }

    pub fn start_timer(&self) {
        self.stop_timer();
        let started = Instant::now();
        let lbl = self.lbl_timer.clone();
        self.lbl_timer.set_visible(true);
        self.lbl_timer.set_label("00:00:00");
        let src = glib::timeout_add_seconds_local(1, move || {
            let secs = started.elapsed().as_secs();
            let h = secs / 3600;
            let m = (secs % 3600) / 60;
            let s = secs % 60;
            lbl.set_label(&format!("{h:02}:{m:02}:{s:02}"));
            glib::Continue(true)
        });
        *self.timer_source.borrow_mut() = Some(src);
    }

    pub fn stop_timer(&self) {
        if let Some(src) = self.timer_source.borrow_mut().take() {
            src.remove();
        }
        self.lbl_timer.set_visible(false);
        self.lbl_timer.set_label("00:00:00");
    }

    pub fn spawn_event_loop(self: &Rc<Self>, evt_rx: async_channel::Receiver<RecorderEvent>) {
        let window = self.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(evt) = evt_rx.recv().await {
                match evt {
                    RecorderEvent::PortalOpened => {
                        window.set_recording_state(UiRecordingState::Preparing);
                        window.set_status("Настройка источника…");
                    }
                    RecorderEvent::ScreenCastReady {
                        fd,
                        node_id,
                        output_path,
                    } => {
                        window.on_screencast_ready(fd, node_id, output_path);
                    }
                    RecorderEvent::RecordingStarted => {
                        window.cancel_force_null_watchdog();
                        window.set_recording_state(UiRecordingState::Recording);
                        window.set_status("Запись…");
                        window.start_timer();
                    }
                    RecorderEvent::RecordingStopped { output_path } => {
                        window.cancel_force_null_watchdog();
                        if let Some(src) = window.bus_guard.borrow_mut().take() {
                            src.remove();
                        }
                        window.pipeline.borrow_mut().take();
                        window.set_recording_state(UiRecordingState::Idle);
                        window.stop_timer();
                        // Remux в целевой контейнер если пользователь выбрал не MKV.
                        let container = window.settings.read().unwrap().container;
                        let final_path =
                            match crate::recorder::output::remux_to(&output_path, container) {
                                Ok(p) => p,
                                Err(e) => {
                                    tracing::warn!(%e, "remux failed, keeping mkv");
                                    window.show_toast(&format!("Remux не удался, оставлен MKV: {e}"));
                                    output_path.clone()
                                }
                            };
                        window.set_status(&format!("Сохранено: {}", final_path.display()));
                        window.show_saved_toast(&final_path);
                        // Закрыть portal-сессию (recorder tokio-задача).
                        let _ = window.cmd_tx.send(UiCommand::StopRequested).await;

                        // Транскрипция: тумблер включён + ключ задан → запускаем.
                        if window.transcription_enabled() {
                            let api_empty = window
                                .settings
                                .read()
                                .unwrap()
                                .openai_api_key
                                .trim()
                                .is_empty();
                            if api_empty {
                                window.show_toast("Укажите API-ключ OpenAI в Настройках");
                            } else {
                                let _ = window
                                    .cmd_tx
                                    .send(UiCommand::TranscribeRequested {
                                        video_path: final_path,
                                    })
                                    .await;
                            }
                        }
                    }
                    RecorderEvent::Error(msg) => {
                        window.cancel_force_null_watchdog();
                        if let Some(src) = window.bus_guard.borrow_mut().take() {
                            src.remove();
                        }
                        if let Some(p) = window.pipeline.borrow_mut().take() {
                            let _ = p.set_state(gst::State::Null);
                        }
                        window.set_recording_state(UiRecordingState::Idle);
                        window.stop_timer();
                        window.set_status("Ошибка записи");
                        window.show_toast(&format!("Ошибка: {msg}"));
                        let _ = window.cmd_tx.send(UiCommand::StopRequested).await;
                    }
                    RecorderEvent::Cancelled => {
                        window.set_recording_state(UiRecordingState::Idle);
                        window.stop_timer();
                        window.set_status("Готов");
                    }
                    RecorderEvent::TranscriptionStarted { .. } => {
                        window.set_stt_busy(true);
                        window.set_status("Распознаю речь…");
                    }
                    RecorderEvent::TranscriptionProgress { part, total, .. } => {
                        if total > 1 {
                            window
                                .set_status(&format!("Распознаю речь… (часть {part} из {total})"));
                        }
                    }
                    RecorderEvent::TranscriptionFinished {
                        text_path,
                        chunks,
                        ..
                    } => {
                        window.set_stt_busy(false);
                        let name = text_path
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        tracing::info!(chunks, path = %text_path.display(), "transcription done");
                        window.set_status(&format!("Расшифровка: {name}"));
                        window.show_saved_text_toast(&text_path);
                    }
                    RecorderEvent::TranscriptionFailed { message, .. } => {
                        window.set_stt_busy(false);
                        tracing::warn!(%message, "transcription failed");
                        window.set_status("Ошибка расшифровки");
                        window.show_toast(&format!("Ошибка расшифровки: {message}"));
                    }
                }
            }
        });
    }

    fn on_screencast_ready(
        &self,
        fd: std::os::fd::RawFd,
        node_id: u32,
        output_path: std::path::PathBuf,
    ) {
        let sources = self.sources_snapshot();
        let settings_snapshot = self.settings.read().unwrap().clone();
        let req = RecordRequest {
            capture_screen: sources.screen,
            capture_system_audio: sources.system_audio,
            capture_mic: sources.microphone,
            output_path: output_path.clone(),
            fd,
            node_id,
            settings: settings_snapshot,
        };
        match build_pipeline(&req) {
            Ok(pipeline) => {
                match attach_bus_watch(&pipeline, self.bus_event_sender(), output_path.clone()) {
                    Ok(guard) => {
                        *self.bus_guard.borrow_mut() = Some(guard);
                    }
                    Err(e) => {
                        tracing::error!(%e, "attach_bus_watch failed");
                        self.set_status(&format!("Ошибка: {e}"));
                        return;
                    }
                }
                if let Err(e) = pipeline_start(&pipeline) {
                    tracing::error!(%e, "pipeline start failed");
                    self.set_status(&format!("Ошибка: {e}"));
                    return;
                }
                *self.pipeline.borrow_mut() = Some(pipeline);
            }
            Err(e) => {
                tracing::error!(%e, "build_pipeline failed");
                self.set_status(&format!("Ошибка: {e}"));
            }
        }
    }

    fn bus_event_sender(&self) -> async_channel::Sender<RecorderEvent> {
        self.evt_tx.clone()
    }

    pub async fn window_identifier(&self) -> WindowIdentifier {
        WindowIdentifier::from_native(&self.window).await
    }

    async fn on_start_stop_clicked(&self) {
        match self.state() {
            UiRecordingState::Idle => {
                self.set_recording_state(UiRecordingState::Preparing);
                self.set_status("Открываю диалог portal…");
                let parent = self.window_identifier().await;
                let cmd = UiCommand::StartRequested {
                    sources: self.sources_snapshot(),
                    parent,
                };
                tracing::info!("start clicked");
                if let Err(err) = self.cmd_tx.send(cmd).await {
                    tracing::error!(%err, "failed to send StartRequested");
                    self.set_recording_state(UiRecordingState::Idle);
                }
            }
            UiRecordingState::Recording => {
                tracing::info!("stop clicked");
                self.initiate_stop();
            }
            UiRecordingState::Preparing | UiRecordingState::Finalizing => {
                tracing::debug!("start/stop clicked in transitional state — ignoring");
            }
        }
    }

    fn initiate_stop(&self) {
        self.set_recording_state(UiRecordingState::Finalizing);
        self.set_status("Сохранение…");
        let pipeline_snapshot = self.pipeline.borrow().clone();
        if let Some(p) = pipeline_snapshot {
            stop_graceful(&p);
            self.arm_force_null_watchdog();
        } else {
            tracing::warn!("stop requested but no pipeline");
            let evt_tx = self.evt_tx.clone();
            glib::MainContext::default().spawn_local(async move {
                let _ = evt_tx.send(RecorderEvent::Cancelled).await;
            });
        }
    }

    fn arm_force_null_watchdog(&self) {
        self.cancel_force_null_watchdog();
        let pipeline_cell = self.pipeline.clone();
        let bus_cell = self.bus_guard.clone();
        let evt_tx = self.evt_tx.clone();
        let src = glib::timeout_add_seconds_local(5, move || {
            if let Some(p) = pipeline_cell.borrow_mut().take() {
                tracing::warn!("EOS timeout (5s), forcing Null");
                let _ = p.set_state(gst::State::Null);
            }
            if let Some(watch) = bus_cell.borrow_mut().take() {
                watch.remove();
            }
            let tx = evt_tx.clone();
            glib::MainContext::default().spawn_local(async move {
                let _ = tx
                    .send(RecorderEvent::Error(
                        "EOS timeout: pipeline не завершился за 5 с, файл может быть обрезан"
                            .into(),
                    ))
                    .await;
            });
            glib::Continue(false)
        });
        *self.force_null_watchdog.borrow_mut() = Some(src);
    }

    fn cancel_force_null_watchdog(&self) {
        if let Some(src) = self.force_null_watchdog.borrow_mut().take() {
            src.remove();
        }
    }

    fn prompt_close_confirmation(self: &Rc<Self>) {
        let dialog = gtk::MessageDialog::builder()
            .transient_for(&self.window)
            .modal(true)
            .buttons(gtk::ButtonsType::None)
            .message_type(gtk::MessageType::Question)
            .text("Идёт запись")
            .secondary_text("Остановить запись и сохранить файл перед выходом?")
            .build();
        dialog.add_button("Отмена", gtk::ResponseType::Cancel);
        dialog.add_button("Остановить и выйти", gtk::ResponseType::Accept);
        dialog.set_default_response(gtk::ResponseType::Cancel);

        let weak_self = Rc::downgrade(self);
        dialog.connect_response(move |d, response| {
            d.close();
            if response != gtk::ResponseType::Accept {
                return;
            }
            let Some(me) = weak_self.upgrade() else {
                return;
            };
            // Инициируем stop; закрываем окно после RecordingStopped.
            me.initiate_stop();
            let win = me.window.clone();
            let state_cell = me.state.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
                if matches!(*state_cell.borrow(), UiRecordingState::Idle) {
                    win.destroy();
                    glib::Continue(false)
                } else {
                    glib::Continue(true)
                }
            });
        });
        dialog.present();
    }
}

/// Строит левую навигационную панель (ListBox) и связывает её со `stack`.
/// Секции Record / Library / AI / Settings переключают видимого ребёнка Stack.
fn build_sidebar(stack: &gtk::Stack) -> gtk::Box {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .vexpand(true)
        .build();
    list.add_css_class("navigation-sidebar");

    // (name, label, icon)
    let main_items: &[(&str, &str, &str)] = &[
        ("record", "Запись", "media-record-symbolic"),
        ("library", "Библиотека", "folder-videos-symbolic"),
        ("ai", "AI", "starred-symbolic"),
    ];
    let settings_items: &[(&str, &str, &str)] =
        &[("settings", "Настройки", "emblem-system-symbolic")];

    let mut rows: Vec<(gtk::ListBoxRow, String)> = Vec::new();
    for (name, label, icon) in main_items {
        let row = make_sidebar_row(label, icon);
        row.set_widget_name(name);
        list.append(&row);
        rows.push((row, (*name).to_owned()));
    }
    let separator = gtk::ListBoxRow::builder().selectable(false).build();
    separator.set_child(Some(&gtk::Separator::new(gtk::Orientation::Horizontal)));
    list.append(&separator);
    for (name, label, icon) in settings_items {
        let row = make_sidebar_row(label, icon);
        row.set_widget_name(name);
        list.append(&row);
        rows.push((row, (*name).to_owned()));
    }

    // По умолчанию активна первая (Запись).
    if let Some((first, _)) = rows.first() {
        list.select_row(Some(first));
    }

    let stack_weak = stack.downgrade();
    list.connect_row_selected(move |_, row| {
        let Some(row) = row else { return };
        let Some(stack) = stack_weak.upgrade() else {
            return;
        };
        let name = row.widget_name();
        if !name.is_empty() {
            stack.set_visible_child_name(name.as_str());
        }
    });

    let container = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .width_request(200)
        .build();
    container.add_css_class("sidebar-pane");

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    container.append(&scroll);

    let footer = gtk::Label::builder()
        .label(&format!("Ralume · v{}", env!("CARGO_PKG_VERSION")))
        .halign(gtk::Align::Start)
        .margin_start(14)
        .margin_end(14)
        .margin_top(10)
        .margin_bottom(10)
        .build();
    footer.add_css_class("caption");
    footer.add_css_class("dim-label");
    container.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    container.append(&footer);

    container
}

fn make_sidebar_row(label: &str, icon: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let hbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .build();
    let image = gtk::Image::from_icon_name(icon);
    image.set_icon_size(gtk::IconSize::Normal);
    let lbl = gtk::Label::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .build();
    hbox.append(&image);
    hbox.append(&lbl);
    row.set_child(Some(&hbox));
    row
}

/// Плейсхолдер для экранов, которые появятся в следующих подфазах.
fn build_placeholder_page(title: &str, subtitle: &str) -> gtk::Widget {
    let page = adw::StatusPage::builder()
        .title(title)
        .description(subtitle)
        .icon_name("emblem-synchronizing-symbolic")
        .vexpand(true)
        .hexpand(true)
        .build();
    page.upcast()
}
