//! Finestrella «Hardware»: resoconto del rilevamento e pulsante per ripetere le prove.

use super::Finestra;
use crate::i18n::t;
use crate::{rileva, stampa};
use adw::prelude::*;
use gtk::{gio, glib};
use std::rc::Rc;

pub fn mostra(f: &Rc<Finestra>) {
    let d = adw::Dialog::builder()
        .title(t("Hardware e encoder"))
        .content_width(720)
        .content_height(640)
        .build();
    let vista = adw::ToolbarView::new();
    vista.add_top_bar(&adw::HeaderBar::new());

    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(6)
        .margin_bottom(14)
        .margin_start(14)
        .margin_end(14)
        .build();
    let testo = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .left_margin(10)
        .right_margin(10)
        .top_margin(8)
        .bottom_margin(8)
        .build();
    let sw = gtk::ScrolledWindow::builder().vexpand(true).child(&testo).build();
    sw.add_css_class("card");
    box_.append(&sw);

    let nota = gtk::Label::builder()
        .wrap(true)
        .xalign(0.0)
        .label(t(
            "Ogni encoder viene provato davvero con pochi fotogrammi. Il risultato resta in memoria \
             finché non cambiano ffmpeg, driver, schede video o kernel.",
        ))
        .build();
    nota.add_css_class("dim-label");
    box_.append(&nota);

    let ripeti = gtk::Button::builder().label(t("Ripeti le prove")).halign(gtk::Align::Center).build();
    ripeti.add_css_class("pill");
    box_.append(&ripeti);
    vista.set_content(Some(&box_));
    d.set_child(Some(&vista));

    let mostra_testo = {
        let testo = testo.clone();
        move |t: &str| testo.buffer().set_text(t)
    };
    match f.rilevamento() {
        Some(r) => mostra_testo(&stampa::resoconto(&r).replace(
            t("usa --forza per ripetere le prove"),
            t("premi «Ripeti le prove» per rifarle"),
        )),
        None => mostra_testo(t("Rilevamento in corso…")),
    }

    let f2 = f.clone();
    ripeti.connect_clicked(move |b| {
        if f2.stato.borrow().in_corso {
            f2.avviso(t("Attendi la fine della compressione"));
            return;
        }
        b.set_sensitive(false);
        b.set_label(t("Prove in corso…"));
        let (b, f3, mostra_testo) = (b.clone(), f2.clone(), mostra_testo.clone());
        glib::spawn_future_local(async move {
            if let Ok(r) = gio::spawn_blocking(|| rileva::rileva_con_cache(true, false)).await {
                mostra_testo(&stampa::resoconto(&r));
                f3.imposta_rilevamento(r);
                f3.log(t("Rilevamento dell'hardware ripetuto."));
            }
            b.set_label(t("Ripeti le prove"));
            b.set_sensitive(true);
        });
    });

    d.present(Some(&f.win));
}
