//! Ricerca di ffmpeg: prima quello indicato, poi quello incluso accanto al programma,
//! infine quello di sistema.

use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfoFfmpeg {
    pub percorso: String,
    pub versione: String,
    /// «incluso», «di sistema» o «variabile COMPRIMI_VIDEO_FFMPEG»
    pub origine: String,
    /// Encoder video compilati in questo ffmpeg (non significa che l'hardware li supporti)
    pub encoder: Vec<String>,
}

impl InfoFfmpeg {
    pub fn ha_encoder(&self, nome: &str) -> bool {
        self.encoder.binary_search_by(|e| e.as_str().cmp(nome)).is_ok()
    }
}

pub fn trova() -> Option<InfoFfmpeg> {
    candidati().into_iter().find_map(|(p, o)| interroga(&p, o))
}

fn candidati() -> Vec<(PathBuf, &'static str)> {
    let mut v = Vec::new();
    if let Some(p) = env::var_os("COMPRIMI_VIDEO_FFMPEG") {
        v.push((PathBuf::from(p), "variabile COMPRIMI_VIDEO_FFMPEG"));
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(d) = exe.parent() {
            v.push((d.join("ffmpeg"), "incluso"));
            v.push((d.join("../lib/comprimi-video/ffmpeg"), "incluso"));
        }
    }
    if let Some(path) = env::var_os("PATH") {
        for d in env::split_paths(&path) {
            v.push((d.join("ffmpeg"), "di sistema"));
        }
    }
    v
}

fn interroga(p: &PathBuf, origine: &str) -> Option<InfoFfmpeg> {
    if !p.is_file() {
        return None;
    }
    let ver = Command::new(p)
        .args(["-hide_banner", "-version"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !ver.status.success() {
        return None;
    }
    let enc = Command::new(p)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let percorso = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
    Some(InfoFfmpeg {
        percorso: percorso.to_string_lossy().into_owned(),
        versione: estrai_versione(&String::from_utf8_lossy(&ver.stdout)),
        origine: origine.to_string(),
        encoder: analizza_encoder(&String::from_utf8_lossy(&enc.stdout)),
    })
}

/// «ffmpeg version 6.1.1-3ubuntu5 Copyright …» → «6.1.1-3ubuntu5»
pub fn estrai_versione(testo: &str) -> String {
    let prima = testo.lines().next().unwrap_or("");
    let mut parole = prima.split_whitespace();
    while let Some(p) = parole.next() {
        if p == "version" {
            return parole.next().unwrap_or("?").to_string();
        }
    }
    "?".to_string()
}

/// Estrae i nomi degli encoder video dall'output di `ffmpeg -encoders`, ordinati.
pub fn analizza_encoder(testo: &str) -> Vec<String> {
    let mut out: Vec<String> = testo
        .lines()
        .skip_while(|r| !r.trim_start().starts_with("------"))
        .skip(1)
        .filter_map(|r| {
            let mut t = r.split_whitespace();
            let flag = t.next()?;
            let nome = t.next()?;
            (flag.len() == 6 && flag.starts_with('V')).then(|| nome.to_string())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn versione() {
        assert_eq!(
            estrai_versione("ffmpeg version 6.1.1-3ubuntu5 Copyright (c) 2000-2023\nbuilt with gcc"),
            "6.1.1-3ubuntu5"
        );
        assert_eq!(estrai_versione("ffmpeg version n7.1 Copyright"), "n7.1");
        assert_eq!(estrai_versione(""), "?");
    }

    #[test]
    fn encoder() {
        let t = "Encoders:\n V..... = Video\n A..... = Audio\n ------\n \
                 V....D libx264              libx264 H.264\n \
                 A....D aac                  AAC\n \
                 V....D hevc_vaapi           H.265/HEVC (VAAPI)\n \
                 V..... libsvtav1            SVT-AV1\n";
        let e = analizza_encoder(t);
        assert_eq!(e, vec!["hevc_vaapi", "libsvtav1", "libx264"]);
        let info = InfoFfmpeg {
            percorso: String::new(),
            versione: String::new(),
            origine: String::new(),
            encoder: e,
        };
        assert!(info.ha_encoder("libx264") && !info.ha_encoder("aac"));
    }
}
