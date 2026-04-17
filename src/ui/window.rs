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

use crate::recorder::{attach_bus_watch, build_video_pipeline, start as pipeline_start, stop_graceful};
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
    Recording,
}

pub struct AppWindow {
    window: adw::ApplicationWindow,
    btn_start_stop: gtk::Button,
    lbl_status: gtk::Label,
    lbl_timer: gtk::Label,
    switch_screen: gtk::Switch,
    switch_sys: gtk::Switch,
    switch_mic: gtk::Switch,
    state: Rc<RefCell<UiRecordingState>>,
    timer_source: Rc<RefCell<Option<glib::SourceId>>>,
    pipeline: Rc<RefCell<Option<gst::Pipeline>>>,
    bus_guard: Rc<RefCell<Option<glib::SourceId>>>,
    cmd_tx: Sender<UiCommand>,
    evt_tx: Sender<RecorderEvent>,
}

impl AppWindow {
    pub fn new(
        app: &adw::Application,
        cmd_tx: Sender<UiCommand>,
        evt_tx: Sender<RecorderEvent>,
    ) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Screen Record")
            .default_width(420)
            .default_height(320)
            .build();

        let header = adw::HeaderBar::new();

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

        group.add(&row_screen);
        group.add(&row_sys);
        group.add(&row_mic);
        root.append(&group);

        let btn_start_stop = gtk::Button::builder()
            .label("Начать запись")
            .halign(gtk::Align::Center)
            .build();
        btn_start_stop.add_css_class("suggested-action");
        btn_start_stop.add_css_class("pill");
        root.append(&btn_start_stop);

        let lbl_timer = gtk::Label::new(Some("00:00"));
        lbl_timer.add_css_class("title-2");
        lbl_timer.add_css_class("timer-label");
        lbl_timer.set_visible(false);
        root.append(&lbl_timer);

        let lbl_status = gtk::Label::new(Some("Готов"));
        lbl_status.add_css_class("dim-label");
        root.append(&lbl_status);

        let main_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        main_box.append(&header);
        main_box.append(&root);
        window.set_content(Some(&main_box));

        let this = Rc::new(Self {
            window,
            btn_start_stop: btn_start_stop.clone(),
            lbl_status,
            lbl_timer,
            switch_screen,
            switch_sys,
            switch_mic,
            state: Rc::new(RefCell::new(UiRecordingState::Idle)),
            timer_source: Rc::new(RefCell::new(None)),
            pipeline: Rc::new(RefCell::new(None)),
            bus_guard: Rc::new(RefCell::new(None)),
            cmd_tx,
            evt_tx,
        });

        let weak_self = Rc::downgrade(&this);
        btn_start_stop.connect_clicked(move |_| {
            let weak = weak_self.clone();
            glib::MainContext::default().spawn_local(async move {
                if let Some(me) = weak.upgrade() {
                    me.on_start_stop_clicked().await;
                }
            });
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
        match state {
            UiRecordingState::Idle => {
                self.btn_start_stop.set_label("Начать запись");
                self.btn_start_stop.remove_css_class("destructive-action");
                self.btn_start_stop.add_css_class("suggested-action");
                self.set_sources_sensitive(true);
            }
            UiRecordingState::Recording => {
                self.btn_start_stop.set_label("Остановить");
                self.btn_start_stop.remove_css_class("suggested-action");
                self.btn_start_stop.add_css_class("destructive-action");
                self.set_sources_sensitive(false);
            }
        }
    }

    fn set_sources_sensitive(&self, sensitive: bool) {
        self.switch_screen.set_sensitive(sensitive);
        self.switch_sys.set_sensitive(sensitive);
        self.switch_mic.set_sensitive(sensitive);
    }

    pub fn set_status(&self, text: &str) {
        self.lbl_status.set_label(text);
    }

    pub fn start_timer(&self) {
        self.stop_timer();
        let started = Instant::now();
        let lbl = self.lbl_timer.clone();
        self.lbl_timer.set_visible(true);
        self.lbl_timer.set_label("00:00");
        let src = glib::timeout_add_seconds_local(1, move || {
            let secs = started.elapsed().as_secs();
            lbl.set_label(&format!("{:02}:{:02}", secs / 60, secs % 60));
            glib::Continue(true)
        });
        *self.timer_source.borrow_mut() = Some(src);
    }

    pub fn stop_timer(&self) {
        if let Some(src) = self.timer_source.borrow_mut().take() {
            src.remove();
        }
        self.lbl_timer.set_visible(false);
        self.lbl_timer.set_label("00:00");
    }

    pub fn spawn_event_loop(self: &Rc<Self>, evt_rx: async_channel::Receiver<RecorderEvent>) {
        let window = self.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(evt) = evt_rx.recv().await {
                match evt {
                    RecorderEvent::PortalOpened => {
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
                        window.set_recording_state(UiRecordingState::Recording);
                        window.set_status("Запись…");
                        window.start_timer();
                    }
                    RecorderEvent::RecordingStopped { output_path } => {
                        if let Some(src) = window.bus_guard.borrow_mut().take() {
                            src.remove();
                        }
                        window.pipeline.borrow_mut().take();
                        window.set_recording_state(UiRecordingState::Idle);
                        window.stop_timer();
                        window.set_status(&format!("Сохранено: {}", output_path.display()));
                        // Закрыть portal-сессию (recorder tokio-задача).
                        let _ = window.cmd_tx.send(UiCommand::StopRequested).await;
                    }
                    RecorderEvent::Error(msg) => {
                        if let Some(src) = window.bus_guard.borrow_mut().take() {
                            src.remove();
                        }
                        if let Some(p) = window.pipeline.borrow_mut().take() {
                            let _ = p.set_state(gst::State::Null);
                        }
                        window.set_recording_state(UiRecordingState::Idle);
                        window.stop_timer();
                        window.set_status(&format!("Ошибка: {}", msg));
                        let _ = window.cmd_tx.send(UiCommand::StopRequested).await;
                    }
                    RecorderEvent::Cancelled => {
                        window.set_recording_state(UiRecordingState::Idle);
                        window.stop_timer();
                        window.set_status("Готов");
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
        match build_video_pipeline(fd, node_id, &output_path) {
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
                tracing::error!(%e, "build_video_pipeline failed");
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
                let parent = self.window_identifier().await;
                let cmd = UiCommand::StartRequested {
                    sources: self.sources_snapshot(),
                    parent,
                };
                tracing::info!("start clicked");
                if let Err(err) = self.cmd_tx.send(cmd).await {
                    tracing::error!(%err, "failed to send StartRequested");
                }
            }
            UiRecordingState::Recording => {
                tracing::info!("stop clicked");
                self.set_status("Сохранение…");
                let pipeline_snapshot = self.pipeline.borrow().clone();
                if let Some(p) = pipeline_snapshot {
                    stop_graceful(&p);
                } else {
                    tracing::warn!("stop clicked but no pipeline");
                    let _ = self.cmd_tx.send(UiCommand::StopRequested).await;
                }
            }
        }
    }
}
