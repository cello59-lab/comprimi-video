//! --installa-menu / --rimuovi-menu: voce nel menu delle applicazioni che punta a questo programma.

use std::path::PathBuf;
use std::process::ExitCode;

const APP_ID: &str = crate::gui::APP_ID;

fn cartella_voci() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    base.join("applications")
}

/// Icona dell'app, inclusa nell'eseguibile e installata con la voce di menu.
pub const ICONA: &str = include_str!("../pacchetto/io.github.cello59_lab.ComprimiVideo.svg");

fn file_icona() -> PathBuf {
    cartella_voci()
        .parent()
        .map(|d| d.join(format!("icons/hicolor/scalable/apps/{APP_ID}.svg")))
        .unwrap_or_default()
}

fn voce() -> PathBuf {
    cartella_voci().join(format!("{APP_ID}.desktop"))
}

/// Voci e icone delle versioni precedenti (Tk, Python/GTK e prime versioni Rust),
/// da togliere per non avere doppioni nel menu.
fn voci_vecchie() -> Vec<PathBuf> {
    vec![
        cartella_voci().join("comprimi-video.desktop"),
        cartella_voci().join("org.local.ComprimiVideo.desktop"),
    ]
}

fn icone_vecchie() -> Vec<PathBuf> {
    let base = file_icona();
    vec![base.with_file_name("org.local.ComprimiVideo.svg")]
}

/// Il file da lanciare: l'AppImage se siamo dentro un'AppImage, altrimenti questo eseguibile.
fn eseguibile() -> PathBuf {
    if let Some(a) = std::env::var_os("APPIMAGE") {
        return PathBuf::from(a);
    }
    std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .unwrap_or_else(|_| PathBuf::from("comprimi-video"))
}

fn aggiorna_database() {
    // programma del sistema: senza le librerie dell'AppImage, che potrebbero non andargli bene
    let _ = std::process::Command::new("update-desktop-database")
        .arg(cartella_voci())
        .env_remove("LD_LIBRARY_PATH")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

pub fn installa() -> ExitCode {
    let percorso = eseguibile();
    let contenuto = format!(
        "[Desktop Entry]\n\
Type=Application\n\
Name=Comprimi Video\n\
GenericName=Compressione video\n\
Comment=Riduci le dimensioni dei video con ffmpeg (HEVC, H.264, AV1; GPU AMD, Intel, NVIDIA o CPU)\n\
Exec=\"{}\" %F\n\
Icon={APP_ID}\n\
MimeType=video/mp4;video/x-matroska;video/quicktime;video/x-msvideo;video/webm;video/x-m4v;video/mp2t;\n\
Terminal=false\n\
StartupNotify=true\n\
StartupWMClass={APP_ID}\n\
Categories=AudioVideo;Video;AudioVideoEditing;\n\
Keywords=ffmpeg;video;compressione;hevc;av1;vaapi;nvenc;\n\
X-GNOME-UsesNotifications=true\n",
        percorso.display()
    );
    let v = voce();
    if let Err(e) = std::fs::create_dir_all(cartella_voci()).and_then(|_| std::fs::write(&v, contenuto)) {
        eprintln!("Impossibile scrivere {}: {e}", v.display());
        return ExitCode::from(1);
    }
    let icona = file_icona();
    if let Some(d) = icona.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&icona, ICONA);
    for v in voci_vecchie() {
        if std::fs::remove_file(&v).is_ok() {
            println!("Rimossa la vecchia voce: {}", v.display());
        }
    }
    for i in icone_vecchie() {
        let _ = std::fs::remove_file(i);
    }
    aggiorna_database();
    println!("Voce di menu creata: {}", v.display());
    println!("Punta a: {}", percorso.display());
    println!("Se sposti il programma, rilancia --installa-menu.");
    ExitCode::SUCCESS
}

pub fn rimuovi() -> ExitCode {
    let mut rimosso = false;
    let _ = std::fs::remove_file(file_icona());
    for i in icone_vecchie() {
        let _ = std::fs::remove_file(i);
    }
    for p in std::iter::once(voce()).chain(voci_vecchie()) {
        if std::fs::remove_file(&p).is_ok() {
            println!("Rimossa: {}", p.display());
            rimosso = true;
        }
    }
    if rimosso {
        aggiorna_database();
    } else {
        println!("Nessuna voce di menu da rimuovere.");
    }
    ExitCode::SUCCESS
}
