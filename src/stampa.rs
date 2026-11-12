//! Resoconto leggibile del rilevamento.

use crate::encoder::{Codec, Famiglia};
use crate::rileva::{Esito, Prova, Rilevamento};
use std::fmt::Write;

fn secondi(ms: u64) -> String {
    format!("{:.1} s", ms as f64 / 1000.0).replace('.', ",")
}

fn riga(p: &Prova) -> String {
    let (segno, nota) = match &p.esito {
        Esito::Funziona => (
            "✓",
            match p.allineamento {
                Some((1, 1)) | None => secondi(p.durata_ms),
                Some((w, h)) if w == h => format!("{} · ritaglia a multipli di {w}", secondi(p.durata_ms)),
                Some((w, h)) => format!("{} · ritaglia a multipli di {w}×{h}", secondi(p.durata_ms)),
            },
        ),
        Esito::NonFunziona(m) => ("✗", m.clone()),
        Esito::NonIncluso => ("–", "non incluso in questo ffmpeg".into()),
        Esito::NonProvato(m) => ("–", m.clone()),
    };
    format!("      {segno} {:<6} {:<12} {nota}", p.codec.nome(), p.encoder)
}

pub fn stampa(r: &Rilevamento) {
    print!("{}", resoconto(r));
}

/// Il resoconto come testo (usato dalla riga di comando e dalla finestra «Hardware»).
pub fn resoconto(r: &Rilevamento) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "Comprimi Video — rilevamento hardware\n");

    match &r.ffmpeg {
        Some(f) => { let _ = writeln!(o, "ffmpeg: {} (versione {}, {})", f.percorso, f.versione, f.origine); }
        None => {
            let _ = writeln!(o, "ffmpeg: NON TROVATO");
            let _ = writeln!(o, "  Installa ffmpeg oppure indica il percorso con COMPRIMI_VIDEO_FFMPEG=/percorso/ffmpeg");
        }
    }

    let _ = writeln!(o, "\nSchede video:");
    if r.gpu.is_empty() {
        let _ = writeln!(o, "  nessuna trovata (si userà solo la CPU)");
    }
    for (i, g) in r.gpu.iter().enumerate() {
        let mut dettagli = vec![];
        if let Some(n) = &g.nodo {
            dettagli.push(n.clone());
        }
        if let Some(d) = &g.driver {
            dettagli.push(format!("driver {d}"));
        }
        if g.principale {
            dettagli.push("scheda principale".into());
        }
        let _ = writeln!(o, "  [{}] {}", i + 1, g.nome);
        if !dettagli.is_empty() {
            let _ = writeln!(o, "      {}", dettagli.join(" · "));
        }
    }

    if r.ffmpeg.is_none() {
        return o;
    }

    let _ = writeln!(o, "\nEncoder provati:");
    let mut gruppi: Vec<(Famiglia, Option<usize>)> = vec![];
    for p in &r.prove {
        if !gruppi.contains(&(p.famiglia, p.gpu)) {
            gruppi.push((p.famiglia, p.gpu));
        }
    }
    for (fam, gpu) in gruppi {
        match gpu {
            Some(i) => { let _ = writeln!(o, "  [{}] {} · {}", i + 1, r.gpu[i].nome, fam.nome()); }
            None => { let _ = writeln!(o, "  CPU · {}", fam.nome()); }
        }
        for p in r.prove.iter().filter(|p| p.famiglia == fam && p.gpu == gpu) {
            let _ = writeln!(o, "{}", riga(p));
        }
    }

    let _ = writeln!(o, "\nScelte consigliate:");
    for c in Codec::TUTTI {
        match r.consigliato(c) {
            Some(p) => {
                let dove = match p.gpu {
                    Some(i) => format!("su [{}] {}", i + 1, r.gpu[i].nome),
                    None => "sulla CPU (più lento)".into(),
                };
                let _ = writeln!(o, "  {:<6} → {} {}", c.nome(), p.encoder, dove);
            }
            None => { let _ = writeln!(o, "  {:<6} → non disponibile", c.nome()); }
        }
    }

    if r.dalla_cache {
        let _ = writeln!(o, "\n(risultato dalla cache; usa --forza per ripetere le prove)");
    }
    o
}
