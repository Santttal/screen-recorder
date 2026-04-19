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
use crate::ui::pages::record::{self as rp, RecordPage};

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
    page: RecordPage,
    sidebar_list: gtk::ListBox,
    stack: gtk::Stack,
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

        // Record-страница (phase 19.a.4) живёт в отдельном модуле.
        let page = rp::build(&settings);

        // Sidebar + content Stack layout (phase 19.a.3).
        // Library / AI / Settings — плейсхолдеры до подфаз 19.a.6 / 19.b / 19.c.
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(150)
            .hexpand(true)
            .vexpand(true)
            .build();
        stack.add_named(&page.root, Some("record"));
        stack.add_named(&build_placeholder_page("Library", "Откроется в фазе 19.b."), Some("library"));
        stack.add_named(&build_placeholder_page("AI", "Откроется в фазе 19.c."), Some("ai"));
        stack.add_named(
            &crate::ui::pages::settings::build(settings.clone()),
            Some("settings"),
        );

        let (sidebar, sidebar_list) = build_sidebar(&stack);

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

        let btn_start_stop = page.btn_start_stop.clone();
        let seg_auto = page.seg_auto.clone();

        let this = Rc::new(Self {
            window,
            page,
            sidebar_list,
            stack,
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

        // Показываем toast-подсказку, если Auto выбрали без API-ключа.
        {
            let weak_self = Rc::downgrade(&this);
            seg_auto.connect_toggled(move |b| {
                if !b.is_active() {
                    return;
                }
                let Some(me) = weak_self.upgrade() else {
                    return;
                };
                if me.settings.read().unwrap().openai_api_key.trim().is_empty() {
                    me.show_toast("Укажите API-ключ OpenAI в Настройках");
                }
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

    /// Переключиться на раздел в sidebar по имени ("record", "library", "ai", "settings").
    pub fn select_view(&self, name: &str) {
        // Найти ListBoxRow с соответствующим widget_name и выбрать его —
        // connect_row_selected уже синхронизирует Stack::visible_child.
        let mut idx = 0;
        while let Some(row) = self.sidebar_list.row_at_index(idx) {
            if row.widget_name() == name {
                self.sidebar_list.select_row(Some(&row));
                return;
            }
            idx += 1;
        }
        // Fallback: если row не нашли (не должно случиться) — просто Stack.
        self.stack.set_visible_child_name(name);
    }

    pub fn sources_snapshot(&self) -> Sources {
        Sources {
            // Источник видео — либо Screen либо Window (см. page.capture_source);
            // capture_screen=true означает «записывай видео-поток», управление
            // MONITOR vs WINDOW для portal произойдёт в 19.a.5.
            screen: true,
            system_audio: self.page.switch_sys.is_active(),
            microphone: self.page.switch_mic.is_active(),
        }
    }

    pub fn state(&self) -> UiRecordingState {
        *self.state.borrow()
    }

    pub fn set_recording_state(&self, state: UiRecordingState) {
        *self.state.borrow_mut() = state;
        self.lbl_rec_dot
            .set_visible(matches!(state, UiRecordingState::Recording));
        let btn = &self.page.btn_start_stop;
        match state {
            UiRecordingState::Idle => {
                btn.set_child(Some(&rp::build_start_content("Начать запись")));
                btn.set_sensitive(true);
                btn.remove_css_class("destructive-action");
                btn.add_css_class("suggested-action");
                self.set_sources_sensitive(true);
            }
            UiRecordingState::Preparing => {
                btn.set_child(Some(&gtk::Label::new(Some("Подготовка…"))));
                btn.set_sensitive(false);
                self.set_sources_sensitive(false);
            }
            UiRecordingState::Recording => {
                btn.set_child(Some(&gtk::Label::new(Some("Остановить"))));
                btn.set_sensitive(true);
                btn.remove_css_class("suggested-action");
                btn.add_css_class("destructive-action");
                self.set_sources_sensitive(false);
            }
            UiRecordingState::Finalizing => {
                btn.set_child(Some(&gtk::Label::new(Some("Сохранение…"))));
                btn.set_sensitive(false);
                self.set_sources_sensitive(false);
            }
        }
    }

    fn set_sources_sensitive(&self, sensitive: bool) {
        self.page.switch_sys.set_sensitive(sensitive);
        self.page.switch_mic.set_sensitive(sensitive);
        self.page.seg_auto.set_sensitive(sensitive);
        self.page.seg_manual.set_sensitive(sensitive);
        self.page.card_screen.set_sensitive(sensitive);
        self.page.card_window.set_sensitive(sensitive);
    }

    pub fn transcription_enabled(&self) -> bool {
        self.page.seg_auto.is_active()
    }

    pub fn set_status(&self, text: &str) {
        self.page.lbl_status.set_label(text);
    }

    pub fn set_stt_busy(&self, busy: bool) {
        self.page.stt_spinner.set_visible(busy);
        if busy {
            self.page.stt_spinner.start();
        } else {
            self.page.stt_spinner.stop();
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
        let lbl = self.page.lbl_timer.clone();
        self.page.lbl_timer.set_visible(true);
        self.page.lbl_timer.set_label("00:00:00");
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
        self.page.lbl_timer.set_visible(false);
        self.page.lbl_timer.set_label("00:00:00");
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
/// Возвращает `(container, list)` — `list` нужен для программной навигации
/// (app.preferences → select "settings").
fn build_sidebar(stack: &gtk::Stack) -> (gtk::Box, gtk::ListBox) {
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

    (container, list)
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
