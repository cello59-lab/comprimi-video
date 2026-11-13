//! Anteprima del video: si segnano le parti da eliminare prima della compressione.

use super::Voce;
use crate::i18n::{num, t, tf};
use crate::tempo::{self, Taglio};
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

struct Dialogo {
    voce: Rc<Voce>,
    al_cambio: Rc<dyn Fn()>,
    toasts: adw::ToastOverlay,
    video: gtk::Video,
    lbl_stato: gtk::Label,
    lbl_tempo: gtk::Label,
    btn_play: gtk::Button,
    btn_inizio: gtk::Button,
    tgl_anteprima: gtk::ToggleButton,
    e_da: adw::EntryRow,
    e_a: adw::EntryRow,
    g_tagli: adw::PreferencesGroup,
    righe: RefCell<Vec<adw::ActionRow>>,
    lbl_riepilogo: gtk::Label,
    inizio: Cell<Option<f64>>,
    attivo: Cell<bool>,
    durata: Cell<f64>,
}

pub fn apri(padre: &impl IsA<gtk::Widget>, voce: Rc<Voce>, al_cambio: Rc<dyn Fn()>) {
    let nome = voce.info.percorso.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let d = adw::Dialog::builder()
        .title(tf("Taglia · {}", &[&nome]))
        .content_width(920)
        .content_height(860)
        .build();
    let toasts = adw::ToastOverlay::new();
    let vista = adw::ToolbarView::new();
    vista.add_top_bar(&adw::HeaderBar::new());
    vista.set_content(Some(&toasts));
    d.set_child(Some(&vista));

    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(6)
        .margin_bottom(14)
        .margin_start(14)
        .margin_end(14)
        .build();
    // scorrevole: con il pannello dei tempi aperto il contenuto può superare lo schermo
    let scorri = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&box_)
        .build();
    toasts.set_child(Some(&scorri));

    // anteprima
    let video = gtk::Video::for_file(Some(&gio::File::for_path(&voce.info.percorso)));
    video.set_vexpand(true);
    video.set_size_request(-1, 300);
    box_.append(&video);
    let lbl_stato = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .label(t("Sposta il cursore, premi «Segna inizio» e poi «Segna fine»: la parte tra i due punti verrà eliminata."))
        .build();
    lbl_stato.add_css_class("dim-label");
    box_.append(&lbl_stato);

    // spostamenti
    let barra = gtk::Box::builder().spacing(6).halign(gtk::Align::Center).build();
    let mut salti = vec![];
    for (etichetta, delta) in [("−5 s".to_string(), -5.0), ("−1 s".to_string(), -1.0), (num("−0.2 s".into()), -0.2)] {
        let b = gtk::Button::with_label(&etichetta);
        barra.append(&b);
        salti.push((b, delta));
    }
    let btn_play = gtk::Button::builder()
        .icon_name("media-playback-start-symbolic")
        .tooltip_text(t("Riproduci / pausa"))
        .build();
    btn_play.add_css_class("suggested-action");
    barra.append(&btn_play);
    for (etichetta, delta) in [(num("+0.2 s".into()), 0.2), ("+1 s".to_string(), 1.0), ("+5 s".to_string(), 5.0)] {
        let b = gtk::Button::with_label(&etichetta);
        barra.append(&b);
        salti.push((b, delta));
    }
    box_.append(&barra);

    let lbl_tempo = gtk::Label::builder().label("00:00.0").margin_top(2).build();
    lbl_tempo.add_css_class("title-3");
    lbl_tempo.add_css_class("numeric");
    box_.append(&lbl_tempo);

    // segni
    let segni = gtk::Box::builder().spacing(8).halign(gtk::Align::Center).build();
    let btn_inizio = gtk::Button::with_label(t("Segna inizio taglio"));
    let btn_fine = gtk::Button::with_label(t("Segna fine ed elimina"));
    btn_fine.add_css_class("destructive-action");
    segni.append(&btn_inizio);
    segni.append(&btn_fine);
    box_.append(&segni);

    let tgl_anteprima = gtk::ToggleButton::builder()
        .label(t("▶ Anteprima del risultato"))
        .halign(gtk::Align::Center)
        .tooltip_text(t("Riproduce il video saltando le parti eliminate, come sarà dopo il taglio"))
        .build();
    box_.append(&tgl_anteprima);

    // tempi a mano
    let esp = adw::ExpanderRow::builder()
        .title(t("Inserisci i tempi a mano"))
        .subtitle(t("Formati: 83, 1:23, 1:23,5 oppure 0:01:23"))
        .build();
    let e_da = adw::EntryRow::builder().title(t("Da")).build();
    let e_a = adw::EntryRow::builder().title(t("A")).build();
    let b_add = gtk::Button::builder().label(t("Aggiungi taglio")).valign(gtk::Align::Center).build();
    let riga_b = adw::ActionRow::builder().title(" ").build();
    riga_b.add_suffix(&b_add);
    esp.add_row(&e_da);
    esp.add_row(&e_a);
    esp.add_row(&riga_b);
    let g_man = adw::PreferencesGroup::new();
    g_man.add(&esp);
    box_.append(&g_man);

    // elenco tagli
    let g_tagli = adw::PreferencesGroup::builder().title(t("Parti eliminate")).build();
    let sw = gtk::ScrolledWindow::builder()
        .min_content_height(110)
        .max_content_height(190)
        .propagate_natural_height(true)
        .child(&g_tagli)
        .build();
    box_.append(&sw);
    let lbl_riepilogo = gtk::Label::builder().xalign(0.0).build();
    lbl_riepilogo.add_css_class("dim-label");
    box_.append(&lbl_riepilogo);

    let fatto = gtk::Button::builder().label(t("Fatto")).halign(gtk::Align::Center).build();
    fatto.add_css_class("suggested-action");
    fatto.add_css_class("pill");
    // fuori dall'area scorrevole: resta visibile anche quando la finestra è bassa
    let barra = gtk::Box::builder()
        .halign(gtk::Align::Center)
        .margin_top(8)
        .margin_bottom(14)
        .build();
    barra.append(&fatto);
    vista.add_bottom_bar(&barra);

    let durata = voce.info.durata;
    let t = Rc::new(Dialogo {
        voce,
        al_cambio,
        toasts,
        video,
        lbl_stato,
        lbl_tempo,
        btn_play,
        btn_inizio,
        tgl_anteprima,
        e_da,
        e_a,
        g_tagli,
        righe: RefCell::new(vec![]),
        lbl_riepilogo,
        inizio: Cell::new(None),
        attivo: Cell::new(true),
        durata: Cell::new(durata),
    });

    for (b, delta) in salti {
        let c = t.clone();
        b.connect_clicked(move |_| c.salta(delta));
    }
    let c = t.clone();
    t.btn_play.connect_clicked(move |_| {
        if let Some(s) = c.stream() {
            s.set_playing(!s.is_playing());
        }
    });
    let c = t.clone();
    t.btn_inizio.connect_clicked(move |_| c.segna_inizio());
    let c = t.clone();
    btn_fine.connect_clicked(move |_| c.segna_fine());
    let c = t.clone();
    t.tgl_anteprima.connect_toggled(move |tg| c.anteprima(tg.is_active()));
    let c = t.clone();
    b_add.connect_clicked(move |_| c.aggiungi_manuale());
    let d2 = d.clone();
    fatto.connect_clicked(move |_| {
        d2.close();
    });
    let c = t.clone();
    d.connect_closed(move |_| {
        c.attivo.set(false);
        if let Some(s) = c.stream() {
            s.set_playing(false);
        }
        c.video.set_file(None::<&gio::File>);
    });
    let c = t.clone();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        if c.tick() {
            glib::ControlFlow::Continue
        } else {
            glib::ControlFlow::Break
        }
    });

    t.ridisegna();
    d.present(Some(padre));
}

impl Dialogo {
    fn stream(&self) -> Option<gtk::MediaStream> {
        self.video.media_stream()
    }

    fn pos(&self) -> f64 {
        self.stream().map_or(0.0, |s| s.timestamp() as f64 / 1e6)
    }

    fn avviso(&self, testo: &str) {
        self.toasts.add_toast(adw::Toast::new(testo));
    }

    fn tick(&self) -> bool {
        if !self.attivo.get() {
            return false;
        }
        let Some(s) = self.stream() else {
            return true; // il file può non essere ancora stato aperto
        };
        if let Some(e) = s.error() {
            self.lbl_stato.set_label(&tf(
                "Anteprima non disponibile ({}). Puoi comunque inserire i tempi a mano.",
                &[&e.message()],
            ));
        }
        if self.durata.get() <= 0.0 && s.duration() > 0 {
            self.durata.set(s.duration() as f64 / 1e6);
        }
        let durata = self.durata.get();
        let pos = self.pos();
        let tagli = self.voce.tagli.borrow().clone();
        if self.tgl_anteprima.is_active() && s.is_playing() {
            // in anteprima salta le parti eliminate
            if let Some(&(a, b)) = tagli.iter().find(|(a, b)| a - 0.03 <= pos && pos < *b) {
                if durata > 0.0 && b >= durata - 0.05 {
                    s.set_playing(false);
                    s.seek((a * 1e6) as i64);
                } else {
                    s.seek((b * 1e6) as i64);
                }
            }
        }
        let mut testo = tempo::fmt_t(pos);
        if durata > 0.0 {
            testo += &format!(" / {}", tempo::fmt_t(durata));
        }
        if !tagli.is_empty() && self.tgl_anteprima.is_active() {
            let tolti: f64 = tagli.iter().filter(|(a, _)| pos > *a).map(|(a, b)| b.min(pos) - a).sum();
            testo += &tf("   ·   nel risultato: {}", &[&tempo::fmt_t(pos - tolti)]);
            if durata > 0.0 {
                testo += &format!(" / {}", tempo::fmt_t(tempo::durata_dopo_tagli(&tagli, durata)));
            }
        }
        self.lbl_tempo.set_label(&testo);
        self.btn_play.set_icon_name(if s.is_playing() {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        });
        true
    }

    fn anteprima(&self, attiva: bool) {
        let Some(s) = self.stream() else { return };
        if attiva {
            if self.voce.tagli.borrow().is_empty() {
                self.avviso(t("Non ci sono ancora tagli da vedere"));
            }
            s.seek(0);
            s.set_playing(true);
        } else {
            s.set_playing(false);
        }
    }

    fn salta(&self, delta: f64) {
        let Some(s) = self.stream() else { return };
        let mut nuovo = (self.pos() + delta).max(0.0);
        if self.durata.get() > 0.0 {
            nuovo = nuovo.min(self.durata.get());
        }
        s.seek((nuovo * 1e6) as i64);
    }

    fn vai(&self, t: f64) {
        if let Some(s) = self.stream() {
            s.seek((t * 1e6) as i64);
        }
    }

    fn segna_inizio(&self) {
        let p = self.pos();
        self.inizio.set(Some(p));
        self.btn_inizio.set_label(&tf("Inizio: {}", &[&tempo::fmt_t(p)]));
        self.avviso(&tf(
            "Inizio del taglio a {}: ora vai alla fine e premi «Segna fine»",
            &[&tempo::fmt_t(p)],
        ));
    }

    fn segna_fine(self: &Rc<Self>) {
        let Some(a) = self.inizio.get() else {
            self.avviso(t("Segna prima l'inizio del taglio"));
            return;
        };
        if self.aggiungi(a, self.pos()) {
            self.inizio.set(None);
            self.btn_inizio.set_label(t("Segna inizio taglio"));
        }
    }

    fn aggiungi_manuale(self: &Rc<Self>) {
        match (tempo::parse_tempo(&self.e_da.text()), tempo::parse_tempo(&self.e_a.text())) {
            (Ok(a), Ok(b)) => {
                if self.aggiungi(a, b) {
                    self.e_da.set_text("");
                    self.e_a.set_text("");
                }
            }
            _ => self.avviso(t("Tempi non validi (esempio: 1:23 o 83,5)")),
        }
    }

    fn aggiungi(self: &Rc<Self>, a: f64, mut b: f64) -> bool {
        if self.durata.get() > 0.0 {
            b = b.min(self.durata.get());
        }
        if b <= a + 0.05 {
            self.avviso(t("La fine deve essere dopo l'inizio"));
            return false;
        }
        let nuovi = {
            let mut v = self.voce.tagli.borrow().clone();
            v.push((a, b));
            tempo::unisci_tagli(&v)
        };
        *self.voce.tagli.borrow_mut() = nuovi;
        self.ridisegna();
        true
    }

    fn togli(self: &Rc<Self>, tg: Taglio) {
        self.voce.tagli.borrow_mut().retain(|x| *x != tg);
        self.ridisegna();
    }

    fn ridisegna(self: &Rc<Self>) {
        for r in self.righe.borrow_mut().drain(..) {
            self.g_tagli.remove(&r);
        }
        let tagli = self.voce.tagli.borrow().clone();
        for tg in &tagli {
            let (a, b) = *tg;
            let riga = adw::ActionRow::builder()
                .title(format!("{}  →  {}", tempo::fmt_t(a), tempo::fmt_t(b)))
                .subtitle(tf("{} s eliminati", &[&num(format!("{:.1}", b - a))]))
                .build();
            let vai = gtk::Button::builder()
                .icon_name("media-seek-forward-symbolic")
                .valign(gtk::Align::Center)
                .tooltip_text(t("Vai all'inizio di questo taglio"))
                .build();
            vai.add_css_class("flat");
            let c = self.clone();
            vai.connect_clicked(move |_| c.vai((a - 1.0).max(0.0)));
            let cest = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .valign(gtk::Align::Center)
                .tooltip_text(t("Annulla questo taglio"))
                .build();
            cest.add_css_class("flat");
            let c = self.clone();
            let tg = *tg;
            cest.connect_clicked(move |_| c.togli(tg));
            riga.add_suffix(&vai);
            riga.add_suffix(&cest);
            self.g_tagli.add(&riga);
            self.righe.borrow_mut().push(riga);
        }
        let durata = self.durata.get();
        if tagli.is_empty() {
            self.lbl_riepilogo.set_label(t("Nessun taglio: il video verrà compresso per intero."));
        } else {
            let dopo = tempo::durata_dopo_tagli(&tagli, durata);
            let mut testo = tf("Eliminati {}", &[&tempo::fmt_t(tempo::durata_tagliata(&tagli, durata))]);
            if durata > 0.0 {
                testo += &tf(" · durata finale circa {}", &[&tempo::fmt_t(dopo)]);
            }
            self.lbl_riepilogo.set_label(&testo);
        }
        (self.al_cambio)();
    }
}
