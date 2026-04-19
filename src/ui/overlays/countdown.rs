//! Countdown-overlay (phase 19.c.3). Отдельное borderless fullscreen окно,
//! показывает крупные цифры 3→2→1, потом закрывается и вызывает колбэк.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use libadwaita as adw;
use gtk4 as gtk;

/// Запустить countdown. По завершении (или при Esc) вызывает `on_finish`.
/// `secs == 0` — сразу вызывает колбэк без показа окна.
pub fn start(
    parent: &adw::ApplicationWindow,
    secs: u32,
    on_finish: impl FnOnce() + 'static,
) {
    if secs == 0 {
        on_finish();
        return;
    }

    let window = gtk::Window::builder()
        .transient_for(parent)
        .decorated(false)
        .resizable(false)
        .modal(true)
        .build();
    window.fullscreen();

    let label = gtk::Label::builder()
        .label(&secs.to_string())
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    label.add_css_class("countdown-number");
    // Inline-стиль: очень крупный шрифт, чтобы быть заметным через оверлеи.
    label.set_attributes(Some(&{
        let attrs = gtk::pango::AttrList::new();
        let mut size = gtk::pango::AttrSize::new(240 * gtk::pango::SCALE);
        size.set_start_index(0);
        attrs.insert(size);
        let mut color = gtk::pango::AttrColor::new_foreground(u16::MAX, u16::MAX, u16::MAX);
        color.set_start_index(0);
        attrs.insert(color);
        attrs
    }));

    let bg = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bg.add_css_class("countdown-bg");
    bg.set_hexpand(true);
    bg.set_vexpand(true);
    bg.append(&label);
    window.set_child(Some(&bg));

    let remaining: Rc<RefCell<u32>> = Rc::new(RefCell::new(secs));
    let on_finish: Rc<RefCell<Option<Box<dyn FnOnce()>>>> =
        Rc::new(RefCell::new(Some(Box::new(on_finish))));

    let win_ref = window.clone();
    let lbl_ref = label.clone();
    let remaining_ref = remaining.clone();
    let on_finish_ref = on_finish.clone();
    glib::timeout_add_seconds_local(1, move || {
        let mut r = remaining_ref.borrow_mut();
        if *r <= 1 {
            win_ref.close();
            drop(r);
            if let Some(cb) = on_finish_ref.borrow_mut().take() {
                cb();
            }
            return glib::Continue(false);
        }
        *r -= 1;
        lbl_ref.set_label(&r.to_string());
        glib::Continue(true)
    });

    // Esc закрывает overlay без запуска записи.
    let ctrl = gtk::EventControllerKey::new();
    let win_ref = window.clone();
    let on_finish_ref_esc = on_finish.clone();
    ctrl.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            win_ref.close();
            if let Some(cb) = on_finish_ref_esc.borrow_mut().take() {
                // Esc отменяет — колбэк НЕ вызываем (MVP phase 19.c.3).
                drop(cb);
            }
            return glib::signal::Inhibit(true);
        }
        glib::signal::Inhibit(false)
    });
    window.add_controller(ctrl);

    window.present();
}
