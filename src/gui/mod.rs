//! Finestra principale (GTK4 + libadwaita): porting della versione Python sopra il nucleo Rust.

mod hardware;
mod taglio;

use crate::encoder::{Codec, Famiglia};
use crate::lavoro::{self, Audio, Backend, Fine, InfoVideo, Opzioni};
use crate::rileva::{self, Esito, Rilevamento};
use crate::tempo::{self, Taglio};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use serde_json::{Map, Value};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const APP_ID: &str = "io.github.cello59_lab.ComprimiVideo";
pub const VERSIONE: &str = env!("CARGO_PKG_VERSION");

// Le etichette sono le stesse della versione Python, così la configurazione resta compatibile.
const RISOLUZIONI: &[&str] = &["Originale", "2160p", "1440p", "1080p", "900p", "720p", "576p", "480p", "360p"];
const FPS_LIMITI: &[&str] = &["Nessun limite", "60", "50", "30", "25", "24"];
const AUDIO: &[&str] = &["Copia (senza ricodifica)", "Ricodifica AAC 128k"];
const SEPARATORI: &[(&str, &str)] = &[("Trattino (-)", "-"), ("Spazio", " "), ("Punto (.)", "."), ("Underscore (_)", "_")];
const CODEC_ORDINE: [Codec; 3] = [Codec::Hevc, Codec::H264, Codec::Av1];

fn etichetta_codec(c: Codec) -> &'static str {
    match c {
        Codec::Hevc => "HEVC (H.265)",
        Codec::H264 => "H.264",
        Codec::Av1 => "AV1",
    }
}

// ------------------------------------------------------------------ configurazione

fn percorso_config() -> PathBuf {
    glib::user_config_dir().join("comprimi-video/config.json")
}

fn carica_config() -> Map<String, Value> {
    std::fs::read_to_string(percorso_config())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn salva_config(m: &Map<String, Value>) {
    let p = percorso_config();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(t) = serde_json::to_string_pretty(m) {
        let _ = std::fs::write(p, t);
    }
}

fn cfg_testo(cfg: &Map<String, Value>, chiave: &str, predef: &str) -> String {
    cfg.get(chiave).and_then(|v| v.as_str()).unwrap_or(predef).to_string()
}

// ------------------------------------------------------------------ piccoli aiuti

fn combo(titolo: &str, sottotitolo: Option<&str>, voci: &[&str], scelta: &str, predef: u32) -> adw::ComboRow {
    let r = adw::ComboRow::builder().title(titolo).model(&gtk::StringList::new(voci)).build();
    if let Some(s) = sottotitolo {
        r.set_subtitle(s);
    }
    r.set_selected(voci.iter().position(|v| *v == scelta).map_or(predef, |i| i as u32));
    r
}

fn testo_scelto(r: &adw::ComboRow) -> String {
    r.selected_item()
        .and_downcast::<gtk::StringObject>()
        .map(|s| s.string().to_string())
        .unwrap_or_default()
}

fn pulsante_icona(icona: &str, suggerimento: &str) -> gtk::Button {
    let b = gtk::Button::builder()
        .icon_name(icona)
        .tooltip_text(suggerimento)
        .valign(gtk::Align::Center)
        .build();
    b.add_css_class("flat");
    b
}

fn id_backend(b: &Backend) -> String {
    match &b.nodo {
        Some(n) => format!("{}@{n}", b.encoder.nome),
        None => b.encoder.nome.to_string(),
    }
}

// ------------------------------------------------------------------ dati

pub struct Voce {
    pub info: InfoVideo,
    pub tagli: RefCell<Vec<Taglio>>,
    fatto: Cell<bool>,
    row: adw::ActionRow,
    stato: gtk::Label,
    barra: gtk::ProgressBar,
    btn: gtk::Button,
    btn_taglia: gtk::Button,
    base_sub: String,
}

impl Voce {
    fn nome(&self) -> String {
        self.info.percorso.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }

    fn aggiorna_sottotitolo(&self) {
        let tagli = self.tagli.borrow();
        let mut sub = self.base_sub.clone();
        if !tagli.is_empty() {
            let n = tempo::unisci_tagli(&tagli).len();
            let tolti = tempo::durata_tagliata(&tagli, self.info.durata);
            sub += &format!(" · ✂ {n} {} (−{})", if n == 1 { "taglio" } else { "tagli" }, tempo::fmt_t(tolti));
        }
        self.row.set_subtitle(&sub);
    }
}

#[derive(Default)]
struct Stato {
    voci: Vec<Rc<Voce>>,
    outdir: Option<PathBuf>,
    ultima: Option<PathBuf>,
    in_corso: bool,
    chiudi_alla_fine: bool,
    rilevamento: Option<Rilevamento>,
    codec_mostrati: Vec<Codec>,
    backends: Vec<Backend>,
    encoder_scelto: String,
    stop: Arc<AtomicBool>,
}

/// Messaggi dal thread di lavoro all'interfaccia.
enum Msg {
    Stato(usize, String, Option<f64>),
    Totale(f64, String),
    Log(String),
    Fatto(usize),
    Fine(usize, bool),
}

pub struct Finestra {
    pub win: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    lista: gtk::ListBox,
    stack: gtk::Stack,
    gruppo: adw::PreferencesGroup,
    btn_file: gtk::Button,
    btn_dir: gtk::Button,
    btn_clear: gtk::Button,
    row_res: adw::ComboRow,
    row_fps: adw::ComboRow,
    row_codec: adw::ComboRow,
    row_encoder: adw::ComboRow,
    row_q: adw::SpinRow,
    row_audio: adw::ComboRow,
    row_pref: adw::EntryRow,
    row_sep: adw::ComboRow,
    row_out: adw::ActionRow,
    log_view: gtk::TextView,
    log_mark: gtk::TextMark,
    progress: gtk::ProgressBar,
    lbl_info: gtk::Label,
    btn_start: gtk::Button,
    btn_stop: gtk::Button,
    stato: RefCell<Stato>,
    /// Vero mentre i menu vengono riempiti dal programma (non salvare, non reagire)
    aggiornando: Cell<bool>,
    pronto: Cell<bool>,
}

// ------------------------------------------------------------------ avvio

thread_local! {
    static FINESTRA: RefCell<Option<Rc<Finestra>>> = const { RefCell::new(None) };
}

fn finestra(app: &adw::Application) -> Rc<Finestra> {
    if let Some(f) = FINESTRA.with(|f| f.borrow().clone()) {
        return f;
    }
    let f = Finestra::nuova(app);
    FINESTRA.with(|c| *c.borrow_mut() = Some(f.clone()));
    f
}

/// Decodificatori video su GPU (VA-API) che GStreamer userebbe per l'anteprima dei tagli.
/// Con alcune combinazioni di driver Mesa e GTK producono artefatti verdi nella riproduzione
/// (verificato su AMD RX 7600): l'anteprima si decodifica quindi sulla CPU. La compressione
/// non ne risente, perché ffmpeg è un processo separato e continua a usare la GPU.
const DECODER_GPU_ESCLUSI: &str = "vah264dec:NONE,vah265dec:NONE,vaav1dec:NONE,vavp9dec:NONE,vavp8dec:NONE,\
vampeg2dec:NONE,vajpegdec:NONE,vaapidecodebin:NONE,vaapih264dec:NONE,vaapih265dec:NONE,\
vaapiav1dec:NONE,vaapivp9dec:NONE,vaapimpeg2dec:NONE";

/// L'icona dell'app è nel tema solo dopo --installa-menu (o dentro l'AppImage):
/// negli altri casi la si scrive nella cache e la si aggiunge ai percorsi delle icone.
fn icona_sempre_disponibile() {
    let Some(display) = gdk::Display::default() else { return };
    let tema = gtk::IconTheme::for_display(&display);
    if tema.has_icon(APP_ID) {
        return;
    }
    let base = glib::user_cache_dir().join("comprimi-video/icone");
    let file = base.join(format!("hicolor/scalable/apps/{APP_ID}.svg"));
    if let Some(d) = file.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if std::fs::write(&file, crate::menu::ICONA).is_ok() {
        tema.add_search_path(&base);
    }
}

pub fn avvia(file: Vec<String>) -> ExitCode {
    // va impostato prima che GStreamer parta (c'è ancora un solo thread, quindi è sicuro);
    // eventuali impostazioni dell'utente nella stessa variabile vengono conservate
    let rank = match std::env::var("GST_PLUGIN_FEATURE_RANK") {
        Ok(v) if !v.is_empty() => format!("{v},{DECODER_GPU_ESCLUSI}"),
        _ => DECODER_GPU_ESCLUSI.to_string(),
    };
    std::env::set_var("GST_PLUGIN_FEATURE_RANK", rank);

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    // dopo l'avvio di GTK: icona delle finestre (quella installata con --installa-menu)
    app.connect_startup(|_| {
        icona_sempre_disponibile();
        gtk::Window::set_default_icon_name(APP_ID);
    });
    app.connect_activate(|app| finestra(app).win.present());
    // «Apri con» da Nautilus o file passati da terminale; funziona anche ad app già aperta
    app.connect_open(|app, files, _| {
        let f = finestra(app);
        f.win.present();
        f.aggiungi(files.iter().filter_map(|f| f.path()).collect());
    });
    let mut argv = vec!["comprimi-video".to_string()];
    argv.extend(file);
    if app.run_with_args(&argv) == glib::ExitCode::SUCCESS {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

impl Finestra {
    fn nuova(app: &adw::Application) -> Rc<Self> {
        let cfg = carica_config();

        let win = adw::ApplicationWindow::builder()
            .application(app)
            .title("Comprimi Video")
            .default_width(760)
            .default_height(900)
            .build();
        let toasts = adw::ToastOverlay::new();
        let vista = adw::ToolbarView::new();

        // barra del titolo
        let header = adw::HeaderBar::new();
        let btn_file = gtk::Button::new();
        btn_file.set_child(Some(
            &adw::ButtonContent::builder().icon_name("list-add-symbolic").label("Aggiungi file").build(),
        ));
        let btn_dir = gtk::Button::builder().icon_name("folder-open-symbolic").tooltip_text("Aggiungi cartella").build();
        let btn_clear = gtk::Button::builder().icon_name("edit-clear-all-symbolic").tooltip_text("Svuota lista").build();
        let btn_hw = gtk::Button::builder()
            .icon_name("video-display-symbolic")
            .tooltip_text("Hardware rilevato ed encoder disponibili")
            .build();
        header.pack_start(&btn_file);
        header.pack_start(&btn_dir);
        // menu principale (☰) con le informazioni sull'app
        let menu = gio::Menu::new();
        menu.append(Some("Informazioni su Comprimi Video"), Some("win.info"));
        let btn_menu = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Menu principale")
            .menu_model(&menu)
            .primary(true)
            .build();
        header.pack_end(&btn_menu);
        header.pack_end(&btn_hw);
        header.pack_end(&btn_clear);
        vista.add_top_bar(&header);

        // contenuto scorrevole
        let box_ = gtk::Box::new(gtk::Orientation::Vertical, 18);
        let clamp = adw::Clamp::builder()
            .maximum_size(760)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .child(&box_)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build();

        let lista = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        lista.add_css_class("boxed-list");
        // pagina vuota fatta a mano: AdwStatusPage ha una sua area scorrevole e,
        // dentro la finestra già scorrevole, verrebbe schiacciata
        let vuoto = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(30)
            .margin_bottom(30)
            .build();
        let img = gtk::Image::builder().icon_name("video-x-generic-symbolic").pixel_size(72).build();
        img.add_css_class("dim-label");
        let tit = gtk::Label::new(Some("Nessun video"));
        tit.add_css_class("title-2");
        let desc = gtk::Label::new(Some("Aggiungi dei file o una cartella, oppure trascinali qui"));
        desc.add_css_class("dim-label");
        vuoto.append(&img);
        vuoto.append(&tit);
        vuoto.append(&desc);
        let stack = gtk::Stack::builder().vhomogeneous(false).build();
        stack.add_named(&vuoto, Some("vuoto"));
        stack.add_named(&lista, Some("lista"));
        box_.append(&stack);

        // impostazioni
        let gruppo = adw::PreferencesGroup::builder().title("Impostazioni").build();
        box_.append(&gruppo);

        let row_res = combo(
            "Risoluzione",
            Some("Solo verso il basso; conta il lato corto, quindi vale anche per i video verticali"),
            RISOLUZIONI,
            &cfg_testo(&cfg, "res", "720p"),
            5,
        );
        let row_fps = combo(
            "Limite fotogrammi (FPS)",
            Some("Applicato solo ai video che superano il limite"),
            FPS_LIMITI,
            &cfg_testo(&cfg, "fps", FPS_LIMITI[0]),
            0,
        );
        let etichette: Vec<&str> = CODEC_ORDINE.iter().map(|c| etichetta_codec(*c)).collect();
        let row_codec = combo(
            "Codec video",
            Some("HEVC equilibrato · H.264 il più compatibile · AV1 file più piccoli, lettori recenti"),
            &etichette,
            &cfg_testo(&cfg, "codec", etichette[0]),
            0,
        );
        // use_subtitle: la scelta (lunga, con il nome della scheda) va nel sottotitolo, intera
        let row_encoder = adw::ComboRow::builder()
            .title("Encoder")
            .use_subtitle(true)
            .model(&gtk::StringList::new(&["Rilevamento dell'hardware in corso…"]))
            .sensitive(false)
            .build();
        let row_q = adw::SpinRow::with_range(15.0, 45.0, 1.0);
        row_q.set_title("Qualità");
        row_q.set_subtitle("Più basso = qualità migliore e file più grande");
        row_q.set_value(cfg.get("q").and_then(|v| v.as_f64()).unwrap_or(32.0));
        let row_audio = combo("Audio", None, AUDIO, &cfg_testo(&cfg, "audio", AUDIO[0]), 0);
        let row_pref = adw::EntryRow::builder().title("Prefisso dei file di output").build();
        row_pref.set_text(&cfg_testo(&cfg, "prefisso", "Znew_"));
        let nomi_sep: Vec<&str> = SEPARATORI.iter().map(|(n, _)| *n).collect();
        let row_sep = combo(
            "Separatore nel suffisso",
            Some("Es.: nome-720p-HEVC-AMD.mp4"),
            &nomi_sep,
            &cfg_testo(&cfg, "sep", nomi_sep[0]),
            0,
        );
        let row_out = adw::ActionRow::builder().title("Cartella di output").build();
        let btn_reset = pulsante_icona("edit-clear-symbolic", "Usa la cartella del file");
        let btn_out = pulsante_icona("folder-open-symbolic", "Scegli cartella");
        row_out.add_suffix(&btn_reset);
        row_out.add_suffix(&btn_out);
        for r in [&row_res, &row_fps, &row_codec, &row_encoder] {
            gruppo.add(r);
        }
        gruppo.add(&row_q);
        gruppo.add(&row_audio);
        gruppo.add(&row_pref);
        gruppo.add(&row_sep);
        gruppo.add(&row_out);

        // log
        let esp = gtk::Expander::new(Some("Dettagli (log)"));
        let log_view = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .left_margin(8)
            .right_margin(8)
            .top_margin(6)
            .bottom_margin(6)
            .build();
        let sw = gtk::ScrolledWindow::builder().min_content_height(160).margin_top(6).child(&log_view).build();
        sw.add_css_class("card");
        esp.set_child(Some(&sw));
        box_.append(&esp);
        let buf = log_view.buffer();
        let log_mark = buf.create_mark(Some("fine"), &buf.end_iter(), false);

        vista.set_content(Some(&scroll));

        // barra inferiore: progresso + azioni
        let fondo = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(8)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        let progress = gtk::ProgressBar::new();
        let lbl_info = gtk::Label::builder().xalign(0.0).label(" ").build();
        lbl_info.add_css_class("dim-label");
        let azioni = gtk::Box::builder().spacing(8).halign(gtk::Align::Center).build();
        let btn_start = gtk::Button::builder().label("Avvia compressione").sensitive(false).build();
        btn_start.add_css_class("suggested-action");
        btn_start.add_css_class("pill");
        let btn_stop = gtk::Button::builder().label("Stop").sensitive(false).build();
        btn_stop.add_css_class("destructive-action");
        btn_stop.add_css_class("pill");
        azioni.append(&btn_start);
        azioni.append(&btn_stop);
        fondo.append(&progress);
        fondo.append(&lbl_info);
        fondo.append(&azioni);
        vista.add_bottom_bar(&fondo);

        toasts.set_child(Some(&vista));
        win.set_content(Some(&toasts));

        let stato = Stato {
            ultima: cfg.get("ultima").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(PathBuf::from),
            encoder_scelto: cfg_testo(&cfg, "encoder", ""),
            ..Stato::default()
        };
        let f = Rc::new(Finestra {
            win,
            toasts,
            lista,
            stack,
            gruppo,
            btn_file,
            btn_dir,
            btn_clear,
            row_res,
            row_fps,
            row_codec,
            row_encoder,
            row_q,
            row_audio,
            row_pref,
            row_sep,
            row_out,
            log_view,
            log_mark,
            progress,
            lbl_info,
            btn_start,
            btn_stop,
            stato: RefCell::new(stato),
            aggiornando: Cell::new(false),
            pronto: Cell::new(false),
        });
        f.imposta_out(cfg.get("outdir").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(PathBuf::from));

        // ---- segnali
        let c = f.clone();
        f.btn_file.connect_clicked(move |_| c.scegli_file());
        let c = f.clone();
        f.btn_dir.connect_clicked(move |_| c.scegli_cartella());
        let c = f.clone();
        f.btn_clear.connect_clicked(move |_| c.svuota());
        let c = f.clone();
        btn_hw.connect_clicked(move |_| hardware::mostra(&c));
        let c = f.clone();
        btn_out.connect_clicked(move |_| c.scegli_out());
        let c = f.clone();
        btn_reset.connect_clicked(move |_| c.imposta_out(None));
        let c = f.clone();
        f.btn_start.connect_clicked(move |_| c.avvia());
        let c = f.clone();
        f.btn_stop.connect_clicked(move |_| c.ferma());

        // ogni modifica si salva subito, come nella versione Python
        for r in [&f.row_res, &f.row_fps, &f.row_audio, &f.row_sep] {
            let c = f.clone();
            r.connect_selected_notify(move |_| c.salva());
        }
        let c = f.clone();
        f.row_codec.connect_selected_notify(move |_| {
            if !c.aggiornando.get() {
                c.aggiorna_encoder();
                c.salva();
            }
        });
        let c = f.clone();
        f.row_encoder.connect_selected_notify(move |_| {
            if !c.aggiornando.get() {
                c.ricorda_encoder();
                c.salva();
            }
        });
        let c = f.clone();
        f.row_q.connect_value_notify(move |_| c.salva());
        let c = f.clone();
        f.row_pref.connect_changed(move |_| c.salva());

        // trascinamento di file e cartelle
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let c = f.clone();
        drop.connect_drop(move |_, valore, _, _| {
            let Ok(lista) = valore.get::<gdk::FileList>() else { return false };
            if c.stato.borrow().in_corso {
                c.avviso("Attendi la fine della compressione per aggiungere file");
                return false;
            }
            c.aggiungi(lista.files().iter().filter_map(|f| f.path()).collect());
            true
        });
        f.win.add_controller(drop);

        let c = f.clone();
        f.win.connect_close_request(move |w| c.chiusura(w));

        let info = gio::SimpleAction::new("info", None);
        let c = f.clone();
        info.connect_activate(move |_, _| c.mostra_info());
        f.win.add_action(&info);

        f.pronto.set(true);
        f.aggiorna_vuoto();
        f.log("Pronto. Aggiungi dei file e premi «Avvia compressione».");

        // rilevamento dell'hardware (immediato se la cache è valida)
        let c = f.clone();
        glib::spawn_future_local(async move {
            if let Ok(r) = gio::spawn_blocking(|| rileva::rileva_con_cache(false, false)).await {
                c.imposta_rilevamento(r);
            }
        });
        f
    }

    // ------------------------------------------------------------------ informazioni

    fn mostra_info(&self) {
        let dentro_appimage = std::env::var("APPIMAGE").is_ok();
        let mut dettagli = format!(
            "Comprimi Video {VERSIONE}\nPacchetto: {}\nGTK {}.{}.{} · libadwaita {}.{}.{}\n",
            if dentro_appimage { "AppImage" } else { "eseguibile" },
            gtk::major_version(),
            gtk::minor_version(),
            gtk::micro_version(),
            adw::major_version(),
            adw::minor_version(),
            adw::micro_version(),
        );
        if let Some(r) = self.rilevamento() {
            dettagli += "\n";
            dettagli += &crate::stampa::resoconto(&r);
        }
        let d = adw::AboutDialog::builder()
            .application_name("Comprimi Video")
            .application_icon(APP_ID)
            .version(VERSIONE)
            .developer_name("cello59")
            .website("https://github.com/cello59-lab/comprimi-video")
            .issue_url("https://github.com/cello59-lab/comprimi-video/issues")
            .license_type(gtk::License::Gpl30)
            .comments(
                "Riduce le dimensioni dei video con ffmpeg: HEVC, H.264 o AV1, \
                 sulla scheda video (AMD, Intel, NVIDIA) oppure sulla CPU, \
                 con tagli e ridimensionamento.",
            )
            .debug_info(dettagli)
            .debug_info_filename("comprimi-video-info.txt")
            .build();
        d.present(Some(&self.win));
    }

    // ------------------------------------------------------------------ messaggi

    pub fn avviso(&self, testo: &str) {
        self.toasts.add_toast(adw::Toast::new(testo));
    }

    pub fn log(&self, testo: &str) {
        let buf = self.log_view.buffer();
        buf.insert(&mut buf.end_iter(), &format!("{testo}\n"));
        buf.move_mark(&self.log_mark, &buf.end_iter());
        self.log_view.scroll_to_mark(&self.log_mark, 0.0, false, 0.0, 0.0);
    }

    fn aggiorna_vuoto(&self) {
        let piena = !self.stato.borrow().voci.is_empty();
        self.stack.set_visible_child_name(if piena { "lista" } else { "vuoto" });
    }

    // ------------------------------------------------------------------ hardware ed encoder

    pub fn rilevamento(&self) -> Option<Rilevamento> {
        self.stato.borrow().rilevamento.clone()
    }

    pub fn imposta_rilevamento(self: &Rc<Self>, r: Rilevamento) {
        let ff = r.ffmpeg.clone();
        self.stato.borrow_mut().rilevamento = Some(r);
        match ff {
            None => {
                self.avviso("ffmpeg non trovato: installalo per poter comprimere");
                self.log("❌ ffmpeg non trovato. Installalo (es. sudo apt install ffmpeg) e riavvia l'app.");
            }
            Some(f) => self.log(&format!("ffmpeg {} ({})", f.versione, f.percorso)),
        }
        self.aggiorna_codec();
        let in_corso = self.stato.borrow().in_corso;
        self.btn_start.set_sensitive(!in_corso && self.stato.borrow().rilevamento.as_ref().is_some_and(|r| r.ffmpeg.is_some()));
    }

    /// Mostra solo i codec che almeno un encoder sa produrre su questa macchina.
    fn aggiorna_codec(self: &Rc<Self>) {
        let precedente = testo_scelto(&self.row_codec);
        let disponibili: Vec<Codec> = {
            let s = self.stato.borrow();
            let r = s.rilevamento.as_ref();
            let d: Vec<Codec> = CODEC_ORDINE
                .iter()
                .copied()
                .filter(|c| r.is_some_and(|r| r.consigliato(*c).is_some()))
                .collect();
            if d.is_empty() { CODEC_ORDINE.to_vec() } else { d }
        };
        let etichette: Vec<&str> = disponibili.iter().map(|c| etichetta_codec(*c)).collect();
        self.aggiornando.set(true);
        self.row_codec.set_model(Some(&gtk::StringList::new(&etichette)));
        self.row_codec
            .set_selected(etichette.iter().position(|e| *e == precedente).unwrap_or(0) as u32);
        self.aggiornando.set(false);
        self.stato.borrow_mut().codec_mostrati = disponibili;
        self.aggiorna_encoder();
    }

    fn codec_scelto(&self) -> Codec {
        let s = self.stato.borrow();
        s.codec_mostrati
            .get(self.row_codec.selected() as usize)
            .copied()
            .unwrap_or(Codec::Hevc)
    }

    /// Riempie la riga «Encoder»: «Automatico» più tutti gli encoder funzionanti per il codec.
    fn aggiorna_encoder(self: &Rc<Self>) {
        let codec = self.codec_scelto();
        let (voci, backends, scelto) = {
            let s = self.stato.borrow();
            let Some(r) = s.rilevamento.as_ref() else { return };
            let auto = r.consigliato(codec).and_then(|p| Backend::da_prova(p, r));
            let mut voci = vec![match &auto {
                Some(b) => format!("Automatico · {}", b.descrizione),
                None => "Nessun encoder funzionante".to_string(),
            }];
            let mut prove: Vec<_> = r.prove.iter().filter(|p| p.codec == codec && p.esito == Esito::Funziona).collect();
            prove.sort_by_key(|p| (p.famiglia == Famiglia::Software, p.gpu));
            let backends: Vec<Backend> = prove.iter().filter_map(|p| Backend::da_prova(p, r)).collect();
            voci.extend(backends.iter().map(|b| b.descrizione.clone()));
            let scelto = backends
                .iter()
                .position(|b| id_backend(b) == s.encoder_scelto)
                .map_or(0, |i| i + 1);
            (voci, backends, scelto)
        };
        let voci_ref: Vec<&str> = voci.iter().map(String::as_str).collect();
        self.aggiornando.set(true);
        self.row_encoder.set_model(Some(&gtk::StringList::new(&voci_ref)));
        self.row_encoder.set_selected(scelto as u32);
        self.row_encoder.set_sensitive(!backends.is_empty());
        self.aggiornando.set(false);
        self.stato.borrow_mut().backends = backends;
    }

    fn ricorda_encoder(&self) {
        let i = self.row_encoder.selected() as usize;
        let mut s = self.stato.borrow_mut();
        s.encoder_scelto = if i == 0 { String::new() } else { s.backends.get(i - 1).map(id_backend).unwrap_or_default() };
    }

    fn backend_scelto(&self) -> Option<Backend> {
        let s = self.stato.borrow();
        let i = self.row_encoder.selected() as usize;
        if i > 0 {
            return s.backends.get(i - 1).cloned();
        }
        let r = s.rilevamento.as_ref()?;
        let codec = s.codec_mostrati.get(self.row_codec.selected() as usize).copied()?;
        r.consigliato(codec).and_then(|p| Backend::da_prova(p, r))
    }

    // ------------------------------------------------------------------ scelta dei file

    fn cartella_iniziale(&self, dlg: &gtk::FileDialog, preferita: Option<&Path>) {
        let s = self.stato.borrow();
        if let Some(p) = preferita.into_iter().chain(s.ultima.as_deref()).find(|p| p.is_dir()) {
            dlg.set_initial_folder(Some(&gio::File::for_path(p)));
        }
    }

    fn scegli_file(self: &Rc<Self>) {
        let filtro = gtk::FileFilter::new();
        filtro.set_name(Some("Video"));
        for e in crate::cli::ESTENSIONI {
            filtro.add_suffix(e);
        }
        let filtri = gio::ListStore::new::<gtk::FileFilter>();
        filtri.append(&filtro);
        let dlg = gtk::FileDialog::builder().title("Scegli i video").build();
        dlg.set_filters(Some(&filtri));
        dlg.set_default_filter(Some(&filtro));
        self.cartella_iniziale(&dlg, None);
        let c = self.clone();
        dlg.open_multiple(Some(&self.win), gio::Cancellable::NONE, move |res| {
            if let Ok(m) = res {
                let paths = (0..m.n_items())
                    .filter_map(|i| m.item(i).and_downcast::<gio::File>())
                    .filter_map(|f| f.path())
                    .collect();
                c.aggiungi(paths);
            }
        });
    }

    fn scegli_cartella(self: &Rc<Self>) {
        let dlg = gtk::FileDialog::builder().title("Scegli una cartella").build();
        self.cartella_iniziale(&dlg, None);
        let c = self.clone();
        dlg.select_folder(Some(&self.win), gio::Cancellable::NONE, move |res| {
            if let Some(p) = res.ok().and_then(|f| f.path()) {
                c.aggiungi(vec![p]);
            }
        });
    }

    fn scegli_out(self: &Rc<Self>) {
        let dlg = gtk::FileDialog::builder().title("Cartella di output").build();
        let attuale = self.stato.borrow().outdir.clone();
        self.cartella_iniziale(&dlg, attuale.as_deref());
        let c = self.clone();
        dlg.select_folder(Some(&self.win), gio::Cancellable::NONE, move |res| {
            if let Some(p) = res.ok().and_then(|f| f.path()) {
                c.imposta_out(Some(p));
            }
        });
    }

    fn imposta_out(&self, p: Option<PathBuf>) {
        self.row_out.set_subtitle(&match &p {
            Some(p) => glib::markup_escape_text(&p.to_string_lossy()).to_string(),
            None => "Stessa cartella del file originale".into(),
        });
        self.stato.borrow_mut().outdir = p;
        self.salva();
    }

    fn ricorda_cartella(&self, paths: &[PathBuf]) {
        let Some(p) = paths.last() else { return };
        let c = if p.is_dir() { Some(p.clone()) } else { p.parent().map(Path::to_path_buf) };
        if let Some(c) = c.filter(|c| c.is_dir()) {
            if self.stato.borrow().ultima.as_ref() != Some(&c) {
                self.stato.borrow_mut().ultima = Some(c);
                self.salva();
            }
        }
    }

    pub fn aggiungi(self: &Rc<Self>, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if self.stato.borrow().in_corso {
            self.avviso("Attendi la fine della compressione per aggiungere file");
            return;
        }
        self.ricorda_cartella(&paths);
        let espansi = crate::cli::espandi(&paths, &self.row_pref.text());
        if espansi.is_empty() {
            self.avviso("Nessun video trovato");
            return;
        }
        let nuovi: Vec<PathBuf> = {
            let s = self.stato.borrow();
            espansi.into_iter().filter(|p| !s.voci.iter().any(|v| &v.info.percorso == p)).collect()
        };
        if nuovi.is_empty() {
            return;
        }
        let ffmpeg = self.stato.borrow().rilevamento.as_ref().and_then(|r| r.ffmpeg.as_ref()).map(|f| f.percorso.clone());
        let c = self.clone();
        glib::spawn_future_local(async move {
            let risultati = gio::spawn_blocking(move || {
                let ffmpeg = ffmpeg.or_else(|| crate::ffmpeg::trova().map(|f| f.percorso));
                let ffprobe = ffmpeg.map_or_else(|| PathBuf::from("ffprobe"), |f| lavoro::ffprobe_per(&f));
                nuovi
                    .into_iter()
                    .map(|p| {
                        let r = lavoro::sonda(&ffprobe, &p);
                        (p, r)
                    })
                    .collect::<Vec<_>>()
            })
            .await;
            for (p, r) in risultati.unwrap_or_default() {
                match r {
                    Ok(info) => c.aggiungi_voce(info),
                    Err(e) => c.log(&format!(
                        "⚠️ {}: {e}, ignorato",
                        p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
                    )),
                }
            }
        });
    }

    fn aggiungi_voce(self: &Rc<Self>, info: InfoVideo) {
        if self.stato.borrow().voci.iter().any(|v| v.info.percorso == info.percorso) {
            return;
        }
        let mut base_sub = format!("{}p", info.lato_corto());
        if info.verticale() {
            base_sub += " verticale";
        }
        if info.fps > 0.0 {
            base_sub += &format!(" · {:.0} fps", info.fps);
        }
        if info.durata > 0.0 {
            base_sub += &format!(" · {}", tempo::fmt_t(info.durata));
        }
        base_sub += &format!(" · {}", lavoro::umano(info.dimensione));

        let nome = info.percorso.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&nome).as_str())
            .subtitle(&base_sub)
            .tooltip_text(info.percorso.to_string_lossy().as_ref())
            .build();
        let barra = gtk::ProgressBar::builder().valign(gtk::Align::Center).visible(false).build();
        barra.set_size_request(110, -1);
        let stato = gtk::Label::new(Some("In attesa"));
        stato.add_css_class("dim-label");
        let btn_taglia = pulsante_icona("edit-cut-symbolic", "Taglia via delle parti del video prima della compressione");
        let btn = pulsante_icona("user-trash-symbolic", "Rimuovi dalla lista");
        row.add_suffix(&barra);
        row.add_suffix(&stato);
        row.add_suffix(&btn_taglia);
        row.add_suffix(&btn);

        let voce = Rc::new(Voce {
            info,
            tagli: RefCell::new(vec![]),
            fatto: Cell::new(false),
            row,
            stato,
            barra,
            btn,
            btn_taglia,
            base_sub,
        });
        let c = self.clone();
        let v = voce.clone();
        voce.btn.connect_clicked(move |_| c.rimuovi(&v));
        let c = self.clone();
        let v = voce.clone();
        voce.btn_taglia.connect_clicked(move |_| {
            if !c.stato.borrow().in_corso {
                let v2 = v.clone();
                taglio::apri(&c.win, v.clone(), Rc::new(move || v2.aggiorna_sottotitolo()));
            }
        });
        self.lista.append(&voce.row);
        self.stato.borrow_mut().voci.push(voce);
        self.aggiorna_vuoto();
    }

    fn rimuovi(&self, voce: &Rc<Voce>) {
        let tolta = {
            let mut s = self.stato.borrow_mut();
            let prima = s.voci.len();
            s.voci.retain(|v| !Rc::ptr_eq(v, voce));
            s.voci.len() != prima
        };
        if tolta {
            self.lista.remove(&voce.row);
            self.aggiorna_vuoto();
        }
    }

    fn svuota(&self) {
        let voci: Vec<Rc<Voce>> = self.stato.borrow().voci.clone();
        for v in voci {
            self.rimuovi(&v);
        }
    }

    // ------------------------------------------------------------------ esecuzione

    fn blocca(&self, in_corso: bool) {
        self.stato.borrow_mut().in_corso = in_corso;
        for w in [self.btn_file.upcast_ref::<gtk::Widget>(), self.btn_dir.upcast_ref(), self.btn_clear.upcast_ref(), self.gruppo.upcast_ref()] {
            w.set_sensitive(!in_corso);
        }
        for v in &self.stato.borrow().voci {
            v.btn.set_sensitive(!in_corso);
            v.btn_taglia.set_sensitive(!in_corso);
        }
        self.btn_start.set_sensitive(!in_corso);
        self.btn_stop.set_sensitive(in_corso);
    }

    fn opzioni(&self) -> Opzioni {
        let res = testo_scelto(&self.row_res);
        let fps = testo_scelto(&self.row_fps);
        let sep = testo_scelto(&self.row_sep);
        Opzioni {
            risoluzione: res.trim_end_matches('p').parse().ok(),
            fps_max: fps.parse().ok(),
            qualita: self.row_q.value() as u8,
            audio: if self.row_audio.selected() == 1 { Audio::Aac } else { Audio::Copia },
            prefisso: self.row_pref.text().to_string(),
            separatore: SEPARATORI.iter().find(|(n, _)| *n == sep).map_or("-", |(_, s)| s).to_string(),
            cartella: self.stato.borrow().outdir.clone(),
        }
    }

    fn avvia(self: &Rc<Self>) {
        let voci: Vec<Rc<Voce>> = self.stato.borrow().voci.clone();
        if voci.is_empty() {
            self.avviso("Aggiungi almeno un video");
            return;
        }
        let lavori: Vec<Rc<Voce>> = voci.into_iter().filter(|v| !v.fatto.get()).collect();
        if lavori.is_empty() {
            self.avviso("Tutti i file sono già stati elaborati");
            return;
        }
        let Some(b) = self.backend_scelto() else {
            self.avviso("Nessun encoder disponibile per questo codec");
            return;
        };
        let Some(ff) = self.stato.borrow().rilevamento.as_ref().and_then(|r| r.ffmpeg.clone()) else {
            self.avviso("ffmpeg non trovato");
            return;
        };
        let opzioni = self.opzioni();
        if let Some(d) = &opzioni.cartella {
            let _ = std::fs::create_dir_all(d);
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.stato.borrow_mut().stop = stop.clone();
        self.progress.set_fraction(0.0);
        self.blocca(true);
        self.log(&format!("Encoder: {}", b.descrizione));

        let elenco: Vec<(InfoVideo, Vec<Taglio>)> =
            lavori.iter().map(|v| (v.info.clone(), v.tagli.borrow().clone())).collect();
        let (tx, rx) = async_channel::unbounded::<Msg>();
        std::thread::spawn(move || lavora(elenco, opzioni, b, ff.percorso, stop, tx));

        let c = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(m) = rx.recv().await {
                match m {
                    Msg::Stato(i, testo, frac) => {
                        let v = &lavori[i];
                        v.stato.set_label(&testo);
                        v.barra.set_visible(frac.is_some());
                        if let Some(f) = frac {
                            v.barra.set_fraction(f);
                        }
                    }
                    Msg::Totale(f, info) => {
                        c.progress.set_fraction(f.min(1.0));
                        c.lbl_info.set_label(if info.is_empty() { " " } else { &info });
                    }
                    Msg::Log(t) => c.log(&t),
                    Msg::Fatto(i) => lavori[i].fatto.set(true),
                    Msg::Fine(fatti, interrotto) => {
                        c.fine(fatti, interrotto);
                        break;
                    }
                }
            }
        });
    }

    fn ferma(&self) {
        self.stato.borrow().stop.store(true, Ordering::Relaxed);
        self.log("Stop richiesto…");
    }

    fn fine(&self, fatti: usize, interrotto: bool) {
        self.blocca(false);
        self.lbl_info.set_label(" ");
        if self.stato.borrow().chiudi_alla_fine {
            self.win.destroy();
            return;
        }
        if interrotto {
            self.log("Interrotto.");
            self.avviso("Compressione interrotta");
            return;
        }
        self.log("Processo terminato.");
        self.avviso("Compressione terminata");
        if let Some(app) = self.win.application() {
            let n = gio::Notification::new("Compressione terminata");
            n.set_body(Some(&format!("File elaborati: {fatti}")));
            app.send_notification(Some("fine"), &n);
        }
    }

    fn chiusura(&self, w: &adw::ApplicationWindow) -> glib::Propagation {
        self.salva();
        if self.stato.borrow().in_corso {
            // ferma ffmpeg e chiudi solo quando il file temporaneo è stato tolto
            self.stato.borrow_mut().chiudi_alla_fine = true;
            self.stato.borrow().stop.store(true, Ordering::Relaxed);
            w.set_visible(false);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    fn salva(&self) {
        if !self.pronto.get() || self.aggiornando.get() {
            return;
        }
        let s = self.stato.borrow();
        let testo = |p: &Option<PathBuf>| p.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let mut m = carica_config(); // conserva eventuali chiavi sconosciute
        m.insert("res".into(), testo_scelto(&self.row_res).into());
        m.insert("fps".into(), testo_scelto(&self.row_fps).into());
        m.insert("codec".into(), testo_scelto(&self.row_codec).into());
        m.insert("encoder".into(), s.encoder_scelto.clone().into());
        m.insert("q".into(), (self.row_q.value() as i64).into());
        m.insert("audio".into(), testo_scelto(&self.row_audio).into());
        m.insert("prefisso".into(), self.row_pref.text().to_string().into());
        m.insert("sep".into(), testo_scelto(&self.row_sep).into());
        m.insert("outdir".into(), testo(&s.outdir).into());
        m.insert("ultima".into(), testo(&s.ultima).into());
        salva_config(&m);
    }
}

// ------------------------------------------------------------------ thread di lavoro

fn lavora(
    lavori: Vec<(InfoVideo, Vec<Taglio>)>,
    o: Opzioni,
    b: Backend,
    ffmpeg: String,
    stop: Arc<AtomicBool>,
    tx: async_channel::Sender<Msg>,
) {
    let invia = |m: Msg| {
        let _ = tx.send_blocking(m);
    };
    let n = lavori.len() as f64;
    let mut fatti = 0;
    for (i, (info, tagli)) in lavori.into_iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let nome = info.percorso.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let piano = match lavoro::pianifica(&info, &tagli, &o, &b) {
            Ok(p) => p,
            Err(e) => {
                invia(Msg::Stato(i, format!("❌ {e}"), None));
                invia(Msg::Log(format!("❌ {nome}: {e}")));
                invia(Msg::Totale((i + 1) as f64 / n, String::new()));
                continue;
            }
        };
        if piano.uscita.exists() {
            invia(Msg::Stato(i, "⏭ Già esistente".into(), None));
            invia(Msg::Totale((i + 1) as f64 / n, String::new()));
            continue;
        }
        invia(Msg::Stato(i, "0%".into(), Some(0.0)));
        invia(Msg::Log(format!(
            "--- {nome} ({}p → {}p, {}, qualità {}) ---",
            info.lato_corto(),
            piano.risoluzione,
            b.encoder.codec.nome(),
            o.qualita
        )));
        for nota in &piano.note {
            invia(Msg::Log(format!("   {nota}")));
        }
        let mut ultimo = -1i64;
        let fine = lavoro::esegui(&ffmpeg, &piano, &stop, |frac, vel| {
            let pm = (frac * 1000.0) as i64;
            if pm != ultimo {
                ultimo = pm;
                let mut testo = format!("File {}/{} · {:.0}%", i + 1, n as usize, frac * 100.0);
                if !vel.is_empty() && vel != "N/A" {
                    testo += &format!(" · velocità {vel}");
                }
                invia(Msg::Stato(i, format!("{:.0}%", frac * 100.0), Some(frac)));
                invia(Msg::Totale((i as f64 + frac) / n, testo));
            }
        });
        match fine {
            Fine::Fatto { dimensione } => {
                fatti += 1;
                let delta = if info.dimensione > 0 {
                    (dimensione as f64 / info.dimensione as f64 - 1.0) * 100.0
                } else {
                    0.0
                };
                invia(Msg::Fatto(i));
                invia(Msg::Stato(i, format!("✅ {} ({delta:+.0}%)", lavoro::umano(dimensione)), None));
                invia(Msg::Log(format!(
                    "✅ {}: {} → {}",
                    piano.uscita.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                    lavoro::umano(info.dimensione),
                    lavoro::umano(dimensione)
                )));
            }
            Fine::Annullato => {
                invia(Msg::Stato(i, "⏹ Annullato".into(), None));
                break;
            }
            Fine::Errore(righe) => {
                invia(Msg::Stato(i, "❌ Errore".into(), None));
                invia(Msg::Log(format!("❌ Errore su {nome}:")));
                for r in righe {
                    invia(Msg::Log(format!("   {r}")));
                }
            }
        }
        invia(Msg::Totale((i + 1) as f64 / n, String::new()));
    }
    invia(Msg::Fine(fatti, stop.load(Ordering::Relaxed)));
}
