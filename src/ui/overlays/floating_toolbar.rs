//! Floating toolbar (phase 19.c.4) — borderless always-on-top окно с таймером,
//! Pause/Stop и переключателями mic/sys. Показывается на время записи.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use libadwaita as adw;
use gtk4 as gtk;

type VoidCallback = Rc<RefCell<Option<Box<dyn Fn()>>>>;

pub struct FloatingToolbar {
    window: gtk::Window,
    timer_label: gtk::Label,
    btn_pause: gtk::Button,
    btn_mic: gtk::ToggleButton,
    btn_sys: gtk::ToggleButton,
    on_pause_toggle: VoidCallback,
    on_stop: VoidCallback,
    on_mic_toggle: Rc<RefCell<Option<Box<dyn Fn(bool)>>>>,
    on_sys_toggle: Rc<RefCell<Option<Box<dyn Fn(bool)>>>>,
    paused: Rc<RefCell<bool>>,
}

impl FloatingToolbar {
    pub fn new(parent: &adw::ApplicationWindow) -> Rc<Self> {
        let window = gtk::Window::builder()
            .transient_for(parent)
            .decorated(false)
            .resizable(false)
            .modal(false)
            .title("Ralume — Запись")
            .build();
        window.set_default_size(1, 1);
        // Prозрачное окно + без CSD-тени/рамки (пункт 2 UX-фидбека).
        window.add_css_class("floating-toolbar-window");

        // Попытка исключить окно из захвата экрана GNOME ScreenCast (пункт 3).
        // `_NET_WM_WINDOW_TYPE = _NET_WM_WINDOW_TYPE_NOTIFICATION` некоторые
        // композиторы трактуют как «не отображать в записи». На GNOME Mutter
        // это не гарантированно работает (ScreenCast захватывает всю композицию),
        // но хуже не станет.
        install_no_capture_hint(&window);

        let hbox = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(10)
            .margin_end(10)
            .build();
        hbox.add_css_class("floating-toolbar");

        // Recording dot + timer.
        let dot = gtk::Label::new(Some("●"));
        dot.add_css_class("recording-dot");
        hbox.append(&dot);

        let timer_label = gtk::Label::new(Some("00:00:00"));
        timer_label.add_css_class("timer-label");
        hbox.append(&timer_label);

        hbox.append(&sep());

        let btn_pause = gtk::Button::builder()
            .icon_name("media-playback-pause-symbolic")
            .tooltip_text("Пауза")
            .build();
        btn_pause.add_css_class("flat");
        hbox.append(&btn_pause);

        let btn_stop = gtk::Button::builder()
            .icon_name("media-playback-stop-symbolic")
            .tooltip_text("Остановить")
            .build();
        btn_stop.add_css_class("destructive-action");
        hbox.append(&btn_stop);

        hbox.append(&sep());

        let btn_mic = gtk::ToggleButton::builder()
            .icon_name("audio-input-microphone-symbolic")
            .tooltip_text("Микрофон")
            .build();
        btn_mic.add_css_class("flat");
        hbox.append(&btn_mic);
        hbox.append(&make_vu_placeholder());

        let btn_sys = gtk::ToggleButton::builder()
            .icon_name("audio-volume-high-symbolic")
            .tooltip_text("Звук системы")
            .build();
        btn_sys.add_css_class("flat");
        hbox.append(&btn_sys);
        hbox.append(&make_vu_placeholder());

        // WindowHandle — позволяет тащить окно мышью за неактивные области тулбара
        // (пункт 1 UX-фидбека).
        let handle = gtk::WindowHandle::new();
        handle.set_child(Some(&hbox));
        window.set_child(Some(&handle));

        let this = Rc::new(Self {
            window,
            timer_label,
            btn_pause,
            btn_mic,
            btn_sys,
            on_pause_toggle: Rc::new(RefCell::new(None)),
            on_stop: Rc::new(RefCell::new(None)),
            on_mic_toggle: Rc::new(RefCell::new(None)),
            on_sys_toggle: Rc::new(RefCell::new(None)),
            paused: Rc::new(RefCell::new(false)),
        });

        {
            let cb = this.on_pause_toggle.clone();
            this.btn_pause.connect_clicked(move |_| {
                if let Some(f) = cb.borrow().as_ref() {
                    f();
                }
            });
        }
        {
            let cb = this.on_stop.clone();
            btn_stop.connect_clicked(move |_| {
                if let Some(f) = cb.borrow().as_ref() {
                    f();
                }
            });
        }
        {
            let cb = this.on_mic_toggle.clone();
            this.btn_mic.connect_toggled(move |b| {
                if let Some(f) = cb.borrow().as_ref() {
                    f(b.is_active());
                }
            });
        }
        {
            let cb = this.on_sys_toggle.clone();
            this.btn_sys.connect_toggled(move |b| {
                if let Some(f) = cb.borrow().as_ref() {
                    f(b.is_active());
                }
            });
        }

        this
    }

    pub fn set_on_pause_toggle(&self, f: impl Fn() + 'static) {
        *self.on_pause_toggle.borrow_mut() = Some(Box::new(f));
    }
    pub fn set_on_stop(&self, f: impl Fn() + 'static) {
        *self.on_stop.borrow_mut() = Some(Box::new(f));
    }
    pub fn set_on_mic_toggle(&self, f: impl Fn(bool) + 'static) {
        *self.on_mic_toggle.borrow_mut() = Some(Box::new(f));
    }
    pub fn set_on_sys_toggle(&self, f: impl Fn(bool) + 'static) {
        *self.on_sys_toggle.borrow_mut() = Some(Box::new(f));
    }

    pub fn show(&self, mic: bool, sys: bool) {
        self.btn_mic.set_active(mic);
        self.btn_sys.set_active(sys);
        self.window.present();
        // Переместить окно в нижнюю центральную часть экрана (отступ ~5% снизу)
        // после того, как WM выделит ему геометрию и id.
        let win = self.window.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
            position_bottom_center(&win, 0.05);
        });
    }
    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    pub fn update_timer(&self, secs: u64) {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        self.timer_label
            .set_label(&format!("{h:02}:{m:02}:{s:02}"));
    }

    pub fn set_paused(&self, paused: bool) {
        *self.paused.borrow_mut() = paused;
        self.btn_pause.set_icon_name(if paused {
            "media-playback-start-symbolic"
        } else {
            "media-playback-pause-symbolic"
        });
        self.btn_pause
            .set_tooltip_text(Some(if paused { "Продолжить" } else { "Пауза" }));
    }

    #[allow(dead_code)]
    pub fn is_paused(&self) -> bool {
        *self.paused.borrow()
    }
}

/// Устанавливает X11 window type = NOTIFICATION для окна тулбара через xprop —
/// часть композиторов трактует NOTIFICATION-окна как «не включать в ScreenCast».
/// На GNOME Mutter (наш дефолт) это не даёт 100% гарантии, т.к. портал снимает
/// всю композицию, но для некоторых настроек (XFCE/KDE с KStatusNotifier, Hyprland)
/// результат — окно не попадает в запись. Best effort.
fn install_no_capture_hint(window: &gtk::Window) {
    let window_ref = window.clone();
    window.connect_map(move |_| {
        // Подождём тик, пока X11 назначит ID окну.
        let w = window_ref.clone();
        glib::idle_add_local_once(move || {
            if let Some(id) = x11_window_id(&w) {
                let _ = std::process::Command::new("xprop")
                    .args([
                        "-id", &id, "-f", "_NET_WM_WINDOW_TYPE", "32a",
                        "-set", "_NET_WM_WINDOW_TYPE",
                        "_NET_WM_WINDOW_TYPE_NOTIFICATION",
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                // Также skip taskbar/pager — удобство.
                let _ = std::process::Command::new("xprop")
                    .args([
                        "-id", &id, "-f", "_NET_WM_STATE", "32a",
                        "-set", "_NET_WM_STATE",
                        "_NET_WM_STATE_SKIP_TASKBAR,_NET_WM_STATE_SKIP_PAGER,_NET_WM_STATE_ABOVE",
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
        });
    });
}

/// Переставить окно в bottom-center первого монитора с `bottom_margin_pct`
/// отступом снизу. Использует xdotool windowmove (X11).
fn position_bottom_center(window: &gtk::Window, bottom_margin_pct: f64) {
    let Some(display) = gtk::gdk::Display::default() else { return };
    let monitors = display.monitors();
    let Some(obj) = monitors.item(0) else { return };
    let Ok(monitor) = obj.downcast::<gtk::gdk::Monitor>() else { return };
    let geom = monitor.geometry();
    let screen_w = geom.width();
    let screen_h = geom.height();

    let (tb_w, tb_h) = {
        let a = window.allocation();
        (a.width().max(window.width()), a.height().max(window.height()))
    };
    if tb_w <= 0 || tb_h <= 0 {
        return;
    }
    let x = geom.x() + (screen_w - tb_w) / 2;
    let margin = (screen_h as f64 * bottom_margin_pct).round() as i32;
    let y = geom.y() + screen_h - tb_h - margin;

    move_window_x11(&window.title().unwrap_or_default(), x, y);
}

/// Переместить X11-окно с указанным WM_NAME в (x, y). Предпочтительно xdotool;
/// fallback — python3 с python-xlib (обычно есть на Ubuntu по умолчанию).
fn move_window_x11(wm_name: &str, x: i32, y: i32) {
    // 1) xdotool.
    if std::process::Command::new("xdotool").arg("--help").output().is_ok() {
        if let Ok(out) = std::process::Command::new("xdotool")
            .args(["search", "--name", wm_name])
            .output()
        {
            if let Some(id) = String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .map(|s| s.trim().to_owned())
            {
                if !id.is_empty() {
                    let _ = std::process::Command::new("xdotool")
                        .args(["windowmove", &id, &x.to_string(), &y.to_string()])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                    return;
                }
            }
        }
    }
    // 2) python3 + python-xlib.
    let script = r#"
import sys
from Xlib.display import Display
d = Display()
root = d.screen().root
target = sys.argv[1]; x = int(sys.argv[2]); y = int(sys.argv[3])
def walk(w, depth=0):
    if depth > 4: return
    try:
        n = w.get_wm_name()
        if isinstance(n, bytes):
            n = n.decode('utf-8', 'replace')
        if n == target:
            w.configure(x=x, y=y)
            d.sync()
            sys.exit(0)
    except Exception:
        pass
    try:
        for c in w.query_tree().children:
            walk(c, depth+1)
    except Exception:
        pass
walk(root)
"#;
    let _ = std::process::Command::new("python3")
        .args(["-c", script, wm_name, &x.to_string(), &y.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Возвращает X11 Window ID как hex-строку для xprop, если мы на X11.
fn x11_window_id(window: &gtk::Window) -> Option<String> {
    // gdk4 surface → через downcast в X11Surface получаем xid. Но эта часть API
    // не всегда экспортирована; пробуем через `xdotool search --name`.
    let out = std::process::Command::new("xdotool")
        .args(["search", "--name", &window.title().unwrap_or_default()])
        .output()
        .ok()?;
    let first = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .to_owned();
    if first.is_empty() {
        None
    } else {
        // xdotool даёт decimal; xprop любит hex или decimal — оба валидны.
        Some(first)
    }
}

fn sep() -> gtk::Separator {
    let s = gtk::Separator::new(gtk::Orientation::Vertical);
    s.set_margin_top(3);
    s.set_margin_bottom(3);
    s
}

/// Пять статичных баров — визуальный placeholder VU (phase 19.c.6 MVP).
/// Реальное подключение GStreamer `level` — будущая работа.
fn make_vu_placeholder() -> gtk::Box {
    let vu = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(2)
        .valign(gtk::Align::Center)
        .build();
    vu.set_width_request(40);
    for i in 0..5 {
        let bar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let h = 4 + i * 3;
        bar.set_size_request(3, h);
        bar.add_css_class(if i >= 3 { "vu-warn" } else { "vu-bar" });
        vu.append(&bar);
    }
    vu
}

// glib для сигналов timer — необходим, но больше ничего не делает.
#[allow(dead_code)]
fn _glib_used() -> Option<glib::SourceId> {
    None
}
