use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use adw::prelude::*;
use async_channel::Sender;
use gtk::gio;
use gtk::glib;
use libadwaita as adw;
use gtk4 as gtk;

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
    cmd_tx: Sender<UiCommand>,
}

impl AppWindow {
    pub fn new(app: &adw::Application, cmd_tx: Sender<UiCommand>) -> Rc<Self> {
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
            cmd_tx,
        });

        let weak_self = Rc::downgrade(&this);
        btn_start_stop.connect_clicked(move |_| {
            if let Some(me) = weak_self.upgrade() {
                me.on_start_stop_clicked();
            }
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
                    RecorderEvent::RecordingStarted => {
                        window.set_recording_state(UiRecordingState::Recording);
                        window.set_status("Запись…");
                        window.start_timer();
                    }
                    RecorderEvent::RecordingStopped { output_path } => {
                        window.set_recording_state(UiRecordingState::Idle);
                        window.stop_timer();
                        window.set_status(&format!("Сохранено: {}", output_path.display()));
                    }
                    RecorderEvent::Error(msg) => {
                        window.set_recording_state(UiRecordingState::Idle);
                        window.stop_timer();
                        window.set_status(&format!("Ошибка: {}", msg));
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

    fn on_start_stop_clicked(&self) {
        let cmd = match self.state() {
            UiRecordingState::Idle => UiCommand::StartRequested(self.sources_snapshot()),
            UiRecordingState::Recording => UiCommand::StopRequested,
        };
        tracing::info!(?cmd, "start/stop clicked");
        if let Err(err) = self.cmd_tx.send_blocking(cmd) {
            tracing::error!(%err, "failed to send UiCommand");
        }
    }
}
