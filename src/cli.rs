//! Uso da terminale: rilevamento e compressione senza finestra.

use crate::encoder::{self, Codec, Famiglia};
use crate::{lavoro, rileva, stampa, tempo};
use crate::lavoro::{Audio, Backend, Fine, Opzioni};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const AIUTO: &str = "\
Comprimi Video

Uso:
  comprimi-video [FILE...]                   apre la finestra (con i file in lista)
  comprimi-video --cli [opzioni] FILE...     comprime da terminale, senza finestra
  comprimi-video --rileva [--forza] [--json]
  comprimi-video --installa-menu | --rimuovi-menu
  comprimi-video --versione

Compressione:
  --codec hevc|h264|av1      codec video (predefinito: hevc)
  --risoluzione N|originale  lato corto massimo, es. 720 (predefinito: 720)
  --fps N                    limite di fotogrammi al secondo (predefinito: nessuno)
  --qualita N                15–45, più basso = migliore e più pesante (predefinito: 32)
  --audio copia|aac          copia l'audio o ricodificalo in AAC 128k (predefinito: copia)
  --uscita CARTELLA          dove salvare (predefinito: accanto all'originale)
  --prefisso TESTO           prefisso dei file prodotti (predefinito: Znew_)
  --separatore C             separatore nel suffisso (predefinito: -)
  --taglia DA-A              elimina una parte, es. --taglia 1:00-1:30 (ripetibile, un solo file)
  --encoder NOME[@NODO]      forza un encoder, es. libx265 o hevc_vaapi@/dev/dri/renderD129
  --prova                    mostra i comandi ffmpeg senza eseguirli

Rilevamento:
  --rileva                   trova schede video ed encoder funzionanti
  --forza                    ripete le prove ignorando la cache
  --json                     resoconto in JSON

ffmpeg viene cercato in quest'ordine: variabile COMPRIMI_VIDEO_FFMPEG,
accanto al programma, nel PATH di sistema.
";

pub const ESTENSIONI: &[&str] = &["mp4", "mkv", "mov", "avi", "webm", "m4v", "mts", "m2ts"];

struct Argomenti {
    rileva: bool,
    forza: bool,
    json: bool,
    prova: bool,
    codec: Codec,
    encoder: Option<String>,
    tagli: Vec<tempo::Taglio>,
    opzioni: Opzioni,
    file: Vec<PathBuf>,
}

fn leggi_argomenti(argomenti: Vec<String>) -> Result<Option<Argomenti>, String> {
    let mut a = Argomenti {
        rileva: false,
        forza: false,
        json: false,
        prova: false,
        codec: Codec::Hevc,
        encoder: None,
        tagli: vec![],
        opzioni: Opzioni::default(),
        file: vec![],
    };
    let mut it = argomenti.into_iter();
    while let Some(arg) = it.next() {
        let mut valore = |nome: &str| it.next().ok_or_else(|| format!("{nome} richiede un valore"));
        match arg.as_str() {
            "-h" | "--help" | "--aiuto" => return Ok(None),
            "--rileva" => a.rileva = true,
            "--forza" => a.forza = true,
            "--json" => a.json = true,
            "--prova" => a.prova = true,
            "--cli" => {}
            "--codec" => {
                a.codec = match valore("--codec")?.to_lowercase().as_str() {
                    "hevc" | "h265" | "h.265" => Codec::Hevc,
                    "h264" | "h.264" | "avc" => Codec::H264,
                    "av1" => Codec::Av1,
                    v => return Err(format!("codec sconosciuto: {v}")),
                }
            }
            "--risoluzione" => {
                let v = valore("--risoluzione")?;
                a.opzioni.risoluzione = match v.trim_end_matches('p') {
                    "originale" | "0" => None,
                    n => Some(n.parse().map_err(|_| format!("risoluzione non valida: {v}"))?),
                }
            }
            "--fps" => {
                let v = valore("--fps")?;
                a.opzioni.fps_max = Some(v.parse().map_err(|_| format!("fps non valido: {v}"))?)
            }
            "--qualita" | "--qualità" => {
                let v = valore("--qualita")?;
                let q: u8 = v.parse().map_err(|_| format!("qualità non valida: {v}"))?;
                if q > 51 {
                    return Err("la qualità va da 0 a 51".into());
                }
                a.opzioni.qualita = q;
            }
            "--audio" => {
                a.opzioni.audio = match valore("--audio")?.as_str() {
                    "copia" => Audio::Copia,
                    "aac" => Audio::Aac,
                    v => return Err(format!("audio non valido: {v} (copia o aac)")),
                }
            }
            "--uscita" => a.opzioni.cartella = Some(PathBuf::from(valore("--uscita")?)),
            "--prefisso" => a.opzioni.prefisso = valore("--prefisso")?,
            "--separatore" => a.opzioni.separatore = valore("--separatore")?,
            "--taglia" => a.tagli.push(tempo::parse_taglio(&valore("--taglia")?)?),
            "--encoder" => a.encoder = Some(valore("--encoder")?),
            s if s.starts_with('-') => return Err(format!("opzione sconosciuta: {s}")),
            _ => a.file.push(PathBuf::from(arg)),
        }
    }
    Ok(Some(a))
}

/// File video singoli o contenuto (non nascosto) delle cartelle, senza i file già prodotti.
pub fn espandi(percorsi: &[PathBuf], prefisso: &str) -> Vec<PathBuf> {
    let video = |p: &Path| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ESTENSIONI.contains(&e.to_lowercase().as_str()))
    };
    let mut out: Vec<PathBuf> = vec![];
    for p in percorsi {
        if p.is_dir() {
            let mut trovati: Vec<PathBuf> = std::fs::read_dir(p)
                .map(|rd| rd.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
            trovati.retain(|f| {
                let nome = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
                f.is_file()
                    && video(f)
                    && !nome.starts_with('.')
                    && !nome.starts_with("tmp_")
                    && (prefisso.is_empty() || !nome.starts_with(prefisso))
            });
            trovati.sort();
            if trovati.is_empty() {
                eprintln!("⚠️  Nessun video in {}", p.display());
            }
            out.extend(trovati);
        } else if p.is_file() && video(p) {
            out.push(p.clone());
        } else if p.is_file() {
            eprintln!("⚠️  Formato non supportato, ignorato: {}", p.display());
        } else {
            eprintln!("⚠️  Non trovato: {}", p.display());
        }
    }
    let mut visti = std::collections::HashSet::new();
    out.retain(|p| visti.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())));
    out
}

/// Sceglie l'encoder: quello forzato, oppure il migliore funzionante per il codec.
fn scegli_backend(a: &Argomenti, r: &rileva::Rilevamento) -> Result<Backend, String> {
    let Some(richiesto) = &a.encoder else {
        let p = r
            .consigliato(a.codec)
            .ok_or_else(|| format!("nessun encoder {} funzionante su questo computer", a.codec.nome()))?;
        return Backend::da_prova(p, r).ok_or_else(|| "encoder sconosciuto".into());
    };
    let (nome, nodo) = match richiesto.split_once('@') {
        Some((n, d)) => (n, Some(d.to_string())),
        None => (richiesto.as_str(), None),
    };
    let def = encoder::cerca(nome).ok_or_else(|| {
        let noti: Vec<_> = encoder::ENCODER.iter().map(|d| d.nome).collect();
        format!("encoder sconosciuto: {nome} (noti: {})", noti.join(", "))
    })?;
    // se il rilevamento lo ha provato, si usa quella prova (con il suo dispositivo)
    let gpu_nodo = |p: &rileva::Prova| p.gpu.and_then(|i| r.gpu[i].nodo.clone());
    if let Some(p) = r
        .prove
        .iter()
        .filter(|p| p.encoder == nome && (nodo.is_none() || gpu_nodo(p) == nodo))
        .min_by_key(|p| p.esito != rileva::Esito::Funziona)
    {
        if p.esito != rileva::Esito::Funziona {
            eprintln!("⚠️  {nome}: il rilevamento dice che non funziona qui, provo comunque");
        }
        return Backend::da_prova(p, r).ok_or_else(|| "encoder sconosciuto".into());
    }
    eprintln!("⚠️  {nome} non è stato verificato dal rilevamento, provo comunque");
    let nodo = match def.famiglia {
        Famiglia::Vaapi => nodo.or_else(|| r.gpu.iter().find(|g| g.accessibile).and_then(|g| g.nodo.clone())),
        _ => None,
    };
    let sigla = match def.famiglia {
        Famiglia::Vaapi => r
            .gpu
            .iter()
            .find(|g| g.nodo == nodo)
            .map_or("GPU", |g| g.produttore.sigla())
            .to_string(),
        Famiglia::Nvenc => "NVENC".into(),
        Famiglia::Software => "CPU".into(),
    };
    Ok(Backend {
        encoder: def,
        descrizione: nome.into(),
        nodo,
        sigla,
        allineamento: Backend::allineamento_predefinito(def.famiglia),
    })
}

pub fn principale(argomenti: Vec<String>) -> ExitCode {
    let a = match leggi_argomenti(argomenti) {
        Ok(Some(a)) => a,
        Ok(None) => {
            print!("{AIUTO}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("{e}\n\nPer l'elenco delle opzioni: comprimi-video --aiuto");
            return ExitCode::from(1);
        }
    };

    if a.rileva {
        let r = rileva::rileva_con_cache(a.forza, !a.json);
        if a.json {
            println!("{}", serde_json::to_string_pretty(&r).expect("json"));
        } else {
            stampa::stampa(&r);
        }
        return if r.ffmpeg.is_some() { ExitCode::SUCCESS } else { ExitCode::from(2) };
    }

    if a.file.is_empty() {
        print!("{AIUTO}");
        return ExitCode::SUCCESS;
    }
    comprimi(a)
}

fn comprimi(a: Argomenti) -> ExitCode {
    let r = rileva::rileva_con_cache(a.forza, true);
    let Some(ff) = r.ffmpeg.clone() else {
        eprintln!("ffmpeg non trovato: installalo oppure indica COMPRIMI_VIDEO_FFMPEG=/percorso/ffmpeg");
        return ExitCode::from(2);
    };
    let backend = match scegli_backend(&a, &r) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let file = espandi(&a.file, &a.opzioni.prefisso);
    if file.is_empty() {
        eprintln!("Nessun video da comprimere.");
        return ExitCode::from(1);
    }
    if !a.tagli.is_empty() && file.len() > 1 {
        eprintln!("--taglia vale per un solo file alla volta");
        return ExitCode::from(1);
    }

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        let _ = ctrlc::set_handler(move || stop.store(true, Ordering::Relaxed));
    }

    let ffprobe = lavoro::ffprobe_per(&ff.percorso);
    let terminale = std::io::stdout().is_terminal();
    let n = file.len();
    let (mut fatti, mut errori, mut prima, mut dopo) = (0, 0, 0u64, 0u64);
    println!("Encoder: {}", backend.descrizione);

    for (i, f) in file.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let nome = f.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        println!("\n[{}/{n}] {nome}", i + 1);
        let v = match lavoro::sonda(&ffprobe, f) {
            Ok(v) => v,
            Err(e) => {
                println!("      ❌ {e}");
                errori += 1;
                continue;
            }
        };
        let piano = match lavoro::pianifica(&v, &a.tagli, &a.opzioni, &backend) {
            Ok(p) => p,
            Err(e) => {
                println!("      ❌ {e}");
                errori += 1;
                continue;
            }
        };
        println!(
            "      {}p → {}p · {} · qualità {}{}",
            v.lato_corto(),
            piano.risoluzione,
            backend.encoder.codec.nome(),
            a.opzioni.qualita,
            if v.verticale() { " · verticale" } else { "" }
        );
        for nota in &piano.note {
            println!("      {nota}");
        }
        if a.prova {
            let cmd: Vec<String> = std::iter::once(ff.percorso.clone())
                .chain(piano.argomenti.iter().map(|s| {
                    if s.contains([' ', ';', '[', '\'', '?']) { format!("'{}'", s.replace('\'', "'\\''")) } else { s.clone() }
                }))
                .collect();
            println!("      {}", cmd.join(" "));
            continue;
        }
        if piano.uscita.exists() {
            println!("      ⏭  già esistente: {}", piano.uscita.display());
            continue;
        }
        let mut ultimo = -1i64;
        let fine = lavoro::esegui(&ff.percorso, &piano, &stop, |frac, vel| {
            let perc = (frac * 100.0) as i64;
            if terminale && perc != ultimo {
                ultimo = perc;
                let vel = if vel.is_empty() || vel == "N/A" { String::new() } else { format!(" · velocità {vel}") };
                print!("\r\x1b[K      {perc}%{vel}");
                let _ = std::io::stdout().flush();
            }
        });
        if terminale {
            print!("\r\x1b[K");
        }
        match fine {
            Fine::Fatto { dimensione } => {
                fatti += 1;
                prima += v.dimensione;
                dopo += dimensione;
                let delta = if v.dimensione > 0 { (dimensione as f64 / v.dimensione as f64 - 1.0) * 100.0 } else { 0.0 };
                println!("      ✅ {} → {} ({delta:+.0}%)", lavoro::umano(v.dimensione), lavoro::umano(dimensione));
                println!("      {}", piano.uscita.display());
            }
            Fine::Annullato => {
                println!("      ⏹  annullato");
                break;
            }
            Fine::Errore(righe) => {
                errori += 1;
                println!("      ❌ errore:");
                for r in righe {
                    println!("         {r}");
                }
            }
        }
    }

    if !a.prova {
        println!();
        if stop.load(Ordering::Relaxed) {
            println!("Interrotto.");
        }
        print!("Fatto: {fatti} file");
        if fatti > 0 {
            print!(", {} → {}", lavoro::umano(prima), lavoro::umano(dopo));
        }
        if errori > 0 {
            print!(", {errori} con errori");
        }
        println!();
    }
    if errori > 0 || stop.load(Ordering::Relaxed) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
