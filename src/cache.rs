//! Cache del rilevamento in ~/.cache/comprimi-video/rilevamento.json.
//! Si invalida da sola quando cambia qualcosa che può cambiare il risultato:
//! ffmpeg, schede, driver, kernel, pacchetti dei driver VAAPI o versione del driver NVIDIA.
//! Conta anche la lingua, perché i motivi dei fallimenti sono salvati già tradotti.

use crate::ffmpeg::InfoFfmpeg;
use crate::gpu::Gpu;
use crate::rileva::{Rilevamento, FORMATO};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const VALIDITA_S: u64 = 30 * 24 * 3600;

const CARTELLE_DRIVER_VA: &[&str] = &[
    "/usr/lib/x86_64-linux-gnu/dri",
    "/usr/lib64/dri",
    "/usr/lib/dri",
    "/usr/lib/aarch64-linux-gnu/dri",
];

fn percorso() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("comprimi-video/rilevamento.json"))
}

fn mtime(p: &Path) -> u64 {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

fn testo(p: &str) -> String {
    fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

pub fn impronta(ff: Option<&InfoFfmpeg>, gpu: &[Gpu]) -> String {
    let mut s = format!(
        "formato={FORMATO};programma={};lingua={};",
        env!("CARGO_PKG_VERSION"),
        crate::i18n::codice()
    );
    // dentro un'AppImage il percorso di ffmpeg cambia a ogni avvio (cartella di montaggio
    // temporanea): conta il file AppImage, che contiene sempre lo stesso ffmpeg
    let appimage = std::env::var("APPIMAGE").ok().filter(|a| {
        ff.is_some_and(|f| Path::new(&f.percorso).starts_with(std::env::var("APPDIR").unwrap_or_default()))
            && !a.is_empty()
    });
    match (ff, appimage) {
        (Some(f), Some(a)) => s += &format!("ffmpeg=appimage:{a}|{}|{};", f.versione, mtime(Path::new(&a))),
        (Some(f), None) => s += &format!(
            "ffmpeg={}|{}|{};",
            f.percorso,
            f.versione,
            mtime(Path::new(&f.percorso))
        ),
        (None, _) => s += "ffmpeg=nessuno;",
    }
    s += &format!("kernel={};", testo("/proc/sys/kernel/osrelease"));
    s += &format!("nvidia={};", testo("/sys/module/nvidia/version"));
    for d in CARTELLE_DRIVER_VA {
        let m = mtime(Path::new(d));
        if m > 0 {
            s += &format!("va={d}@{m};");
        }
    }
    for g in gpu {
        s += &format!(
            "gpu={}|{:04x}:{:04x}|{}|{};",
            g.nodo.as_deref().unwrap_or("-"),
            g.vendor_id,
            g.device_id,
            g.driver.as_deref().unwrap_or("-"),
            g.accessibile
        );
    }
    s
}

pub fn leggi(impronta: &str) -> Option<Rilevamento> {
    let dati = fs::read_to_string(percorso()?).ok()?;
    let r: Rilevamento = serde_json::from_str(&dati).ok()?;
    let ora = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    (r.formato == FORMATO && r.impronta == impronta && ora.saturating_sub(r.creato) < VALIDITA_S)
        .then_some(r)
}

pub fn scrivi(r: &Rilevamento) {
    let Some(p) = percorso() else { return };
    if let Some(d) = p.parent() {
        let _ = fs::create_dir_all(d);
    }
    if let Ok(j) = serde_json::to_string_pretty(r) {
        // scrittura atomica: file temporaneo e rinomina
        let tmp = p.with_extension("json.tmp");
        if fs::write(&tmp, j).is_ok() {
            let _ = fs::rename(&tmp, &p);
        }
    }
}
