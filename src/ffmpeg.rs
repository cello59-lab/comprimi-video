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

/// Sceglie ffmpeg: quello indicato con la variabile vince sempre; fra quello incluso
/// (nell'AppImage) e quello di sistema si preferisce il sistema solo se è di una versione
/// principale più recente e ha gli encoder essenziali. Le versioni più nuove gestiscono
/// meglio le GPU (es. dalla 7 le dimensioni con gli encoder AMD).
pub fn trova() -> Option<InfoFfmpeg> {
    let mut incluso: Option<InfoFfmpeg> = None;
    let mut sistema: Option<InfoFfmpeg> = None;
    for (p, o) in candidati() {
        let gia = match o {
            "incluso" => incluso.is_some(),
            "di sistema" => sistema.is_some(),
            _ => false,
        };
        if gia {
            continue;
        }
        let Some(i) = interroga(&p, o) else { continue };
        match o {
            "incluso" => incluso = Some(i),
            "di sistema" => sistema = Some(i),
            _ => return Some(i), // variabile COMPRIMI_VIDEO_FFMPEG
        }
    }
    match (incluso, sistema) {
        (Some(i), Some(s)) if preferisci_sistema(&i, &s) => Some(s),
        (Some(i), _) => Some(i),
        (None, s) => s,
    }
}

fn preferisci_sistema(incluso: &InfoFfmpeg, sistema: &InfoFfmpeg) -> bool {
    let essenziali = ["libx264", "libx265"].iter().all(|e| sistema.ha_encoder(e));
    match (versione_principale(&incluso.versione), versione_principale(&sistema.versione)) {
        (Some(vi), Some(vs)) => essenziali && vs > vi,
        _ => false,
    }
}

/// «8.0.1-3ubuntu2» → 8, «n7.1» → 7; None per le versioni di sviluppo («N-11234-g…»).
pub fn versione_principale(v: &str) -> Option<u32> {
    let v = v.strip_prefix('n').unwrap_or(v);
    let cifre: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
    cifre.parse().ok()
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
    fn versioni() {
        assert_eq!(versione_principale("8.0.1-3ubuntu2+esm5"), Some(8));
        assert_eq!(versione_principale("6.1.1-3ubuntu5"), Some(6));
        assert_eq!(versione_principale("n7.1"), Some(7));
        assert_eq!(versione_principale("N-118000-gabcdef"), None);
        let info = |v: &str, enc: &[&str]| InfoFfmpeg {
            percorso: String::new(),
            versione: v.into(),
            origine: String::new(),
            encoder: {
                let mut e: Vec<String> = enc.iter().map(|s| s.to_string()).collect();
                e.sort();
                e
            },
        };
        let incluso = info("6.1.1-3ubuntu5", &["libx264", "libx265"]);
        assert!(preferisci_sistema(&incluso, &info("8.0.1", &["hevc_vaapi", "libx264", "libx265"])));
        assert!(!preferisci_sistema(&incluso, &info("6.1.2", &["libx264", "libx265"])));
        // ffmpeg «ridotto» di alcune distribuzioni, senza x264/x265: meglio quello incluso
        assert!(!preferisci_sistema(&incluso, &info("7.1", &["h264_vaapi"])));
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
