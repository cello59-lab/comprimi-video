//! Il nucleo: sonda del video, piano del comando ffmpeg ed esecuzione con avanzamento.
//! Nessuna dipendenza dall'interfaccia: lo userà sia la riga di comando sia la finestra GTK.

use crate::encoder::{Codec, DefEncoder, Famiglia};
use crate::i18n::{num, t, tf};
use crate::rileva::{Prova, Rilevamento};
use crate::tempo::{self, Taglio};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// ------------------------------------------------------------------ sonda

#[derive(Debug, Clone)]
pub struct InfoVideo {
    pub percorso: PathBuf,
    /// Dimensioni come appaiono a schermo (rotazione dei telefoni già applicata)
    pub larghezza: u32,
    pub altezza: u32,
    pub durata: f64,
    pub fps: f64,
    pub dimensione: u64,
    /// Codec del primo flusso audio, se c'è
    pub audio: Option<String>,
}

impl InfoVideo {
    /// Il numero dei «p»: 1080 sia per 1920×1080 sia per un video verticale 1080×1920.
    pub fn lato_corto(&self) -> u32 {
        self.larghezza.min(self.altezza)
    }
    pub fn verticale(&self) -> bool {
        self.altezza > self.larghezza
    }
}

/// ffprobe accanto a ffmpeg (stessa installazione), altrimenti quello nel PATH.
pub fn ffprobe_per(ffmpeg: &str) -> PathBuf {
    let vicino = Path::new(ffmpeg).with_file_name("ffprobe");
    if vicino.is_file() {
        vicino
    } else {
        PathBuf::from("ffprobe")
    }
}

fn frazione(v: &serde_json::Value) -> f64 {
    let s = v.as_str().unwrap_or("");
    match s.split_once('/') {
        Some((a, b)) => match (a.parse::<f64>(), b.parse::<f64>()) {
            (Ok(a), Ok(b)) if b != 0.0 => a / b,
            _ => 0.0,
        },
        None => s.parse().unwrap_or(0.0),
    }
}

pub fn sonda(ffprobe: &Path, percorso: &Path) -> Result<InfoVideo, String> {
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,codec_name,width,height,avg_frame_rate,r_frame_rate\
             :stream_disposition=attached_pic:stream_side_data=rotation:stream_tags=rotate\
             :format=duration",
            "-of",
            "json",
        ])
        .arg(percorso)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| tf("impossibile avviare ffprobe: {}", &[&e]))?;
    let dimensione = std::fs::metadata(percorso).map(|m| m.len()).unwrap_or(0);
    analizza_sonda(&String::from_utf8_lossy(&out.stdout), percorso, dimensione)
}

pub fn analizza_sonda(json: &str, percorso: &Path, dimensione: u64) -> Result<InfoVideo, String> {
    let d: serde_json::Value = serde_json::from_str(json).map_err(|_| t("file non leggibile").to_string())?;
    let flussi = d["streams"].as_array().cloned().unwrap_or_default();
    let video = flussi
        .iter()
        .find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_i64().unwrap_or(0) == 0)
        .ok_or_else(|| t("nessun flusso video trovato").to_string())?;
    let (w, h) = (video["width"].as_u64().unwrap_or(0) as u32, video["height"].as_u64().unwrap_or(0) as u32);
    if w == 0 || h == 0 {
        return Err(t("risoluzione non rilevata").into());
    }
    let rotazione = video["side_data_list"]
        .as_array()
        .and_then(|l| l.iter().find_map(|s| s["rotation"].as_f64()))
        .or_else(|| video["tags"]["rotate"].as_str().and_then(|r| r.parse().ok()))
        .unwrap_or(0.0);
    let ruotato = (rotazione.abs().round() as i64) % 180 == 90;
    let fps = match frazione(&video["avg_frame_rate"]) {
        f if f > 0.0 => f,
        _ => frazione(&video["r_frame_rate"]),
    };
    let audio = flussi
        .iter()
        .find(|s| s["codec_type"] == "audio")
        .map(|s| s["codec_name"].as_str().unwrap_or("?").to_string());
    Ok(InfoVideo {
        percorso: percorso.to_path_buf(),
        larghezza: if ruotato { h } else { w },
        altezza: if ruotato { w } else { h },
        durata: d["format"]["duration"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0),
        fps,
        dimensione,
        audio,
    })
}

// ------------------------------------------------------------------ opzioni e backend

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Audio {
    Copia,
    Aac,
}

#[derive(Debug, Clone)]
pub struct Opzioni {
    /// Lato corto massimo (720 = «720p»); None = originale
    pub risoluzione: Option<u32>,
    pub fps_max: Option<u32>,
    /// Scala 0–51, più basso = qualità migliore
    pub qualita: u8,
    pub audio: Audio,
    pub prefisso: String,
    pub separatore: String,
    /// None = stessa cartella del file originale
    pub cartella: Option<PathBuf>,
}

impl Default for Opzioni {
    fn default() -> Self {
        Opzioni {
            risoluzione: Some(720),
            fps_max: None,
            qualita: 32,
            audio: Audio::Copia,
            prefisso: "Znew_".into(),
            separatore: "-".into(),
            cartella: None,
        }
    }
}

/// Un encoder scelto, con il dispositivo e la sigla da mettere nel nome del file.
#[derive(Debug, Clone)]
pub struct Backend {
    pub encoder: &'static DefEncoder,
    pub nodo: Option<String>,
    pub sigla: String,
    pub descrizione: String,
    /// Multipli (larghezza, altezza) a cui ritagliare il video prima dell'encoder
    pub allineamento: (u32, u32),
}

impl Backend {
    /// Allineamento da usare quando non è stato misurato.
    pub fn allineamento_predefinito(f: Famiglia) -> (u32, u32) {
        match f {
            Famiglia::Vaapi => (16, 16),
            _ => (1, 1),
        }
    }

    pub fn da_prova(p: &Prova, r: &Rilevamento) -> Option<Backend> {
        let encoder = crate::encoder::cerca(&p.encoder)?;
        let gpu = p.gpu.and_then(|i| r.gpu.get(i));
        let sigla = match encoder.famiglia {
            Famiglia::Vaapi => gpu.map_or("GPU", |g| g.produttore.sigla()).to_string(),
            Famiglia::Nvenc => "NVENC".into(),
            Famiglia::Software => "CPU".into(),
        };
        let descrizione = match gpu {
            Some(g) => tf("{} su {}", &[&encoder.nome, &g.nome]),
            None => tf("{} sulla CPU", &[&encoder.nome]),
        };
        Some(Backend {
            encoder,
            nodo: if encoder.famiglia == Famiglia::Vaapi { gpu.and_then(|g| g.nodo.clone()) } else { None },
            sigla,
            descrizione,
            allineamento: p.allineamento.unwrap_or_else(|| Backend::allineamento_predefinito(encoder.famiglia)),
        })
    }
}

// ------------------------------------------------------------------ piano

#[derive(Debug, Clone)]
pub struct Piano {
    pub argomenti: Vec<String>,
    pub uscita: PathBuf,
    pub temporaneo: PathBuf,
    /// Lato corto del risultato (il numero dei «p»)
    pub risoluzione: u32,
    /// Durata attesa del risultato, per la percentuale
    pub durata: f64,
    pub note: Vec<String>,
    /// Dimensioni del video che arrivano all'encoder prima di un eventuale ritaglio
    pub previste: (u32, u32),
}

/// Esito della prova dell'encoder con le dimensioni esatte di un video.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Verifica {
    /// Dimensioni provate (quelle del video da comprimere)
    pub originali: (u32, u32),
    /// Cosa ne ha fatto l'encoder
    pub prodotte: (u32, u32),
    /// Dimensioni da dare all'encoder perché le rispetti (uguali alle originali se va bene così)
    pub finali: (u32, u32),
}

/// Dimensioni da provare, dalla più grande: le originali, poi ritagli ai multipli
/// di 8…128 di una o di entrambe le dimensioni.
pub fn candidati_ritaglio((w, h): (u32, u32)) -> Vec<(u32, u32)> {
    let multipli = [1u32, 8, 16, 32, 64, 128];
    let mut c: Vec<(u32, u32)> = multipli
        .iter()
        .flat_map(|aw| multipli.iter().map(move |ah| (w / aw * aw, h / ah * ah)))
        .filter(|(cw, ch)| *cw >= 16 && *ch >= 16 && cw % 2 == 0 && ch % 2 == 0)
        .collect();
    c.sort_by_key(|(cw, ch)| std::cmp::Reverse(cw * ch));
    c.dedup();
    c
}

/// Prova l'encoder con le dimensioni del video e, se le altera, cerca il ritaglio più piccolo
/// che rispetta. Serve per gli encoder su GPU, che con alcune dimensioni producono righe o
/// colonne in più (banda verde): es. 854×480 → 894×480 con l'HEVC AMD. I risultati restano
/// in memoria finché il programma è aperto, così in una serie di video uguali la prova si fa una volta.
pub fn verifica_dimensioni(ffmpeg: &str, b: &Backend, dim: (u32, u32)) -> Option<Verifica> {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    if b.encoder.famiglia == Famiglia::Software {
        return None; // gli encoder software rispettano sempre le dimensioni
    }
    static RICORDATE: OnceLock<Mutex<HashMap<String, Option<Verifica>>>> = OnceLock::new();
    let chiave = format!("{ffmpeg}|{}|{}|{}x{}", b.encoder.nome, b.nodo.as_deref().unwrap_or("-"), dim.0, dim.1);
    let ricordate = RICORDATE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = ricordate.lock().unwrap().get(&chiave) {
        return *v;
    }
    let mut prodotte = None;
    let mut esito = None;
    for (i, c) in candidati_ritaglio(dim).into_iter().take(8).enumerate() {
        match crate::rileva::prova_dimensioni(ffmpeg, b.encoder, b.nodo.as_deref(), c) {
            Some(d) if d == c => {
                esito = Some(Verifica { originali: dim, prodotte: prodotte.unwrap_or(d), finali: c });
                break;
            }
            Some(d) if i == 0 => prodotte = Some(d),
            Some(_) => {}
            // l'encoder non riesce nemmeno a fare la prova: si torna alla regola del rilevamento
            None if i == 0 => break,
            None => {}
        }
    }
    ricordate.lock().unwrap().insert(chiave, esito);
    esito
}

/// Come `pianifica`, ma prima prova l'encoder con le dimensioni esatte del video
/// (vedi `verifica_dimensioni`) e ritaglia solo se serve davvero.
pub fn pianifica_verificata(
    ffmpeg: &str,
    v: &InfoVideo,
    tagli: &[Taglio],
    o: &Opzioni,
    b: &Backend,
) -> Result<Piano, String> {
    let piano = pianifica(v, tagli, o, b)?;
    match verifica_dimensioni(ffmpeg, b, piano.previste) {
        Some(vf) => pianifica_con(v, tagli, o, b, Some(vf)),
        None => Ok(piano),
    }
}

/// Codec audio che si possono copiare così come sono in un file MP4.
fn audio_compatibile_mp4(codec: &str) -> bool {
    matches!(codec, "aac" | "mp3" | "ac3" | "eac3" | "opus" | "alac" | "flac" | "mp2")
}

pub fn nome_uscita(v: &InfoVideo, o: &Opzioni, b: &Backend, risoluzione: u32) -> (PathBuf, PathBuf) {
    let base = v.percorso.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let cartella = o
        .cartella
        .clone()
        .unwrap_or_else(|| v.percorso.parent().map(Path::to_path_buf).unwrap_or_default());
    let sep = &o.separatore;
    let suffisso = [format!("{risoluzione}p"), b.encoder.codec.sigla().to_string(), b.sigla.clone()].join(sep);
    let nome = format!("{}{base}{sep}{suffisso}.mp4", o.prefisso);
    (cartella.join(&nome), cartella.join(format!("tmp_{nome}")))
}

pub fn pianifica(v: &InfoVideo, tagli: &[Taglio], o: &Opzioni, b: &Backend) -> Result<Piano, String> {
    pianifica_con(v, tagli, o, b, None)
}

/// `verifica`: esito della prova dell'encoder con le dimensioni del video; se c'è, decide lei
/// il ritaglio, altrimenti vale l'allineamento misurato nel rilevamento.
pub fn pianifica_con(
    v: &InfoVideo,
    tagli: &[Taglio],
    o: &Opzioni,
    b: &Backend,
    verifica: Option<Verifica>,
) -> Result<Piano, String> {
    let mut note = Vec::new();
    let mut filtri: Vec<String> = Vec::new();

    if let Some(lim) = o.fps_max {
        if v.fps > lim as f64 + 0.5 {
            filtri.push(format!("fps={lim}"));
            note.push(format!("{:.0} → {lim} fps", v.fps));
        }
    }
    let (mut risoluzione, mut lw, mut lh) = (v.lato_corto(), v.larghezza, v.altezza);
    if let Some(t) = o.risoluzione {
        if v.lato_corto() > t {
            // il ridimensionamento agisce sul lato corto, così anche i video verticali diventano «720p»
            let lungo = (v.larghezza.max(v.altezza) as f64 * t as f64 / v.lato_corto() as f64).round() as u32 / 2 * 2;
            if v.verticale() {
                filtri.push(format!("scale={t}:-2"));
                (lw, lh) = (t, lungo);
            } else {
                filtri.push(format!("scale=-2:{t}"));
                (lw, lh) = (lungo, t);
            }
            risoluzione = t;
        }
    }
    // Alcuni encoder su GPU (es. AMD con ffmpeg 6) arrotondano le dimensioni a un multiplo
    // (16, 32…) senza dire al lettore di togliere le righe in più, che appaiono come una banda
    // verde (es. 1920×1080 → 1920×1088). Il multiplo si misura nel rilevamento; qui si ritaglia
    // al centro a quel multiplo, così all'encoder non resta nulla da arrotondare.
    let naturali = (lw, lh);
    // HEVC e AV1 lavorano a blocchi di 8 pixel: con dimensioni non multiple di 8 il video viene
    // codificato più grande, con l'istruzione per il lettore di nascondere il superfluo. Molti
    // lettori (VLC, televisori) la applicano male con la decodifica su GPU: 854×480, codificato
    // a 856, appariva 894×480 con una banda verde. Con dimensioni multiple di 8 non serve
    // nessuna istruzione e nessun lettore può sbagliare. Per H.264 l'istruzione serve a ogni
    // video 1080p e tutti la gestiscono bene: lì si lascia tutto com'è.
    let base = if b.encoder.codec == Codec::H264 { naturali } else { (lw / 8 * 8, lh / 8 * 8) };
    let previste = base;
    let (aw, ah) = b.allineamento;
    let (finali, nota_encoder) = match verifica.filter(|vf| vf.originali == base) {
        Some(vf) => (
            vf.finali,
            (vf.prodotte != vf.originali).then(|| {
                tf(
                    "{}×{}: l'encoder lo trasformerebbe in {}×{} (banda verde); ritagliato a {}×{}",
                    &[&base.0, &base.1, &vf.prodotte.0, &vf.prodotte.1, &vf.finali.0, &vf.finali.1],
                )
            }),
        ),
        // nessuna prova con le dimensioni del video: vale l'allineamento misurato nel rilevamento
        None => {
            let f = (base.0 / aw * aw, base.1 / ah * ah);
            let multiplo = if aw == ah { aw.to_string() } else { format!("{aw}×{ah}") };
            (
                f,
                (f != base).then(|| {
                    tf(
                        "{}×{} ritagliato a {}×{}: questo encoder vuole dimensioni multiple di {}",
                        &[&base.0, &base.1, &f.0, &f.1, &multiplo],
                    )
                }),
            )
        }
    };
    if finali != naturali {
        let (cw, ch) = finali;
        // min(): se il ridimensionamento di ffmpeg desse un pixel in meno, il ritaglio non fallisce
        filtri.push(format!("crop='min(iw,{cw})':'min(ih,{ch})'"));
        if base != naturali {
            note.push(tf(
                "{}×{} ritagliato a {}×{}: con HEVC e AV1 le dimensioni multiple di 8 vengono mostrate bene da tutti i lettori",
                &[&lw, &lh, &base.0, &base.1],
            ));
        }
        note.extend(nota_encoder);
        risoluzione = cw.min(ch);
    }
    filtri.push(b.encoder.filtro_finale().into());
    let filtro = filtri.join(",");

    let (uscita, temporaneo) = nome_uscita(v, o, b, risoluzione);

    let mut a: Vec<String> = ["-hide_banner", "-y", "-nostdin"].iter().map(|s| s.to_string()).collect();
    a.extend(b.encoder.argomenti_dispositivo(b.nodo.as_deref()));
    a.push("-i".into());
    a.push(v.percorso.to_string_lossy().into_owned());

    let mut ricodifica = o.audio == Audio::Aac;
    if let Some(c) = &v.audio {
        if !ricodifica && !audio_compatibile_mp4(c) {
            ricodifica = true;
            note.push(tf("audio {} non adatto a MP4: ricodificato in AAC", &[c]));
        }
    }

    let mut durata = v.durata;
    if tagli.is_empty() {
        a.extend(["-map", "0:V:0", "-map", "0:a?", "-vf"].iter().map(|s| s.to_string()));
        a.push(filtro);
    } else {
        let tenute = tempo::parti_tenute(tagli, v.durata);
        if tenute.is_empty() {
            return Err(t("i tagli eliminano tutto il video").into());
        }
        let con_audio = v.audio.is_some();
        durata = tempo::durata_dopo_tagli(tagli, v.durata);
        let n = tempo::unisci_tagli(tagli).len();
        note.push(tf(
            "✂ {} {} − {}, durata finale {}",
            &[
                &n,
                &if n == 1 { t("parte eliminata") } else { t("parti eliminate") },
                &tempo::fmt_t(tempo::durata_tagliata(tagli, v.durata)),
                &tempo::fmt_t(durata),
            ],
        ));
        a.push("-filter_complex".into());
        a.push(tempo::filtro_con_tagli(&tenute, con_audio, &filtro));
        a.extend(["-map", "[vo]"].iter().map(|s| s.to_string()));
        if con_audio {
            a.extend(["-map", "[ac]"].iter().map(|s| s.to_string()));
            if !ricodifica {
                ricodifica = true; // l'audio tagliato non si può copiare
                note.push(t("con i tagli l'audio viene ricodificato in AAC").into());
            }
        }
    }

    a.push("-c:v".into());
    a.push(b.encoder.nome.into());
    a.extend(b.encoder.argomenti_qualita(o.qualita));
    if ricodifica {
        a.extend(["-c:a", "aac", "-b:a", "128k"].iter().map(|s| s.to_string()));
    } else {
        a.extend(["-c:a", "copy"].iter().map(|s| s.to_string()));
    }
    // niente capitoli: ffmpeg li copia in una traccia a parte e, se nell'originale l'ultimo
    // capitolo ha una fine sbagliata (succede negli MKV), il file risulta lungo quanto lui
    // (es. 21 ore per un film di 2 h 25); con i tagli sarebbero comunque fuori posto
    a.extend(["-map_chapters", "-1"].iter().map(|s| s.to_string()));
    // faststart: il video parte subito anche in streaming e nelle anteprime dei telefoni
    a.extend(["-movflags", "+faststart", "-progress", "pipe:1", "-nostats"].iter().map(|s| s.to_string()));
    a.push(temporaneo.to_string_lossy().into_owned());

    Ok(Piano { argomenti: a, uscita, temporaneo, risoluzione, durata, note, previste })
}

// ------------------------------------------------------------------ esecuzione

#[derive(Debug)]
pub enum Fine {
    Fatto { dimensione: u64 },
    Errore(Vec<String>),
    Annullato,
}

/// Avanzamento: frazione 0–1 e velocità riportata da ffmpeg (es. «3.2x»).
pub fn esegui(ffmpeg: &str, piano: &Piano, stop: &AtomicBool, mut avanzamento: impl FnMut(f64, &str)) -> Fine {
    if let Some(d) = piano.uscita.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let mut figlio = match Command::new(ffmpeg)
        .args(&piano.argomenti)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(f) => f,
        Err(e) => return Fine::Errore(vec![tf("impossibile avviare ffmpeg: {}", &[&e])]),
    };

    let ultime: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    let lettore = {
        let ultime = Arc::clone(&ultime);
        let err = figlio.stderr.take().expect("stderr");
        std::thread::spawn(move || {
            for riga in BufReader::new(err).lines().map_while(Result::ok) {
                let r = riga.trim().to_string();
                if r.is_empty() {
                    continue;
                }
                let mut u = ultime.lock().unwrap();
                u.push_back(r);
                if u.len() > 12 {
                    u.pop_front();
                }
            }
        })
    };

    let out = figlio.stdout.take().expect("stdout");
    // un guardiano ferma ffmpeg appena arriva lo stop, anche se in quel momento non scrive nulla
    let figlio = Arc::new(Mutex::new(figlio));
    let finito = Arc::new(AtomicBool::new(false));
    std::thread::scope(|s| {
        s.spawn(|| {
            while !finito.load(Ordering::Relaxed) {
                if stop.load(Ordering::Relaxed) {
                    let _ = figlio.lock().unwrap().kill();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });

        let mut velocita = String::new();
        for riga in BufReader::new(out).lines().map_while(Result::ok) {
            let Some((k, v)) = riga.trim().split_once('=') else { continue };
            match k {
                "out_time_us" | "out_time_ms" => {
                    // nonostante il nome, anche out_time_ms è in microsecondi
                    if let Ok(us) = v.parse::<i64>() {
                        if piano.durata > 0.0 {
                            avanzamento((us as f64 / 1e6 / piano.durata).clamp(0.0, 1.0), &velocita);
                        }
                    }
                }
                "speed" => velocita = v.trim().to_string(),
                _ => {}
            }
        }
        finito.store(true, Ordering::Relaxed);
    });
    let stato = figlio.lock().unwrap().wait();
    let _ = lettore.join();

    if stop.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&piano.temporaneo);
        return Fine::Annullato;
    }
    match stato {
        Ok(s) if s.success() && piano.temporaneo.is_file() => match std::fs::rename(&piano.temporaneo, &piano.uscita) {
            Ok(()) => Fine::Fatto {
                dimensione: std::fs::metadata(&piano.uscita).map(|m| m.len()).unwrap_or(0),
            },
            Err(e) => Fine::Errore(vec![tf("impossibile rinominare il file: {}", &[&e])]),
        },
        _ => {
            let _ = std::fs::remove_file(&piano.temporaneo);
            let u = ultime.lock().unwrap();
            Fine::Errore(if u.is_empty() { vec![t("ffmpeg è terminato con un errore").into()] } else { u.iter().cloned().collect() })
        }
    }
}

pub fn umano(n: u64) -> String {
    let mut v = n as f64;
    for unita in ["B", "KB", "MB", "GB", "TB"] {
        if v < 1024.0 {
            return if unita == "B" { format!("{v:.0} B") } else { num(format!("{v:.1} {unita}")) };
        }
        v /= 1024.0;
    }
    num(format!("{v:.1} PB"))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::encoder::cerca;

    fn video(w: u32, h: u32, audio: Option<&str>) -> InfoVideo {
        InfoVideo {
            percorso: PathBuf::from("/video/gara.mov"),
            larghezza: w,
            altezza: h,
            durata: 60.0,
            fps: 59.94,
            dimensione: 1000,
            audio: audio.map(String::from),
        }
    }

    fn backend(enc: &str, sigla: &str) -> Backend {
        Backend {
            encoder: cerca(enc).unwrap(),
            nodo: enc.ends_with("_vaapi").then(|| "/dev/dri/renderD128".to_string()),
            sigla: sigla.into(),
            descrizione: String::new(),
            allineamento: Backend::allineamento_predefinito(cerca(enc).unwrap().famiglia),
        }
    }

    fn cmd(p: &Piano) -> String {
        p.argomenti.join(" ")
    }

    #[test]
    fn vaapi_orizzontale() {
        let p = pianifica(&video(1920, 1080, Some("aac")), &[], &Opzioni::default(), &backend("hevc_vaapi", "AMD")).unwrap();
        let c = cmd(&p);
        assert!(c.starts_with("-hide_banner -y -nostdin -vaapi_device /dev/dri/renderD128 -i /video/gara.mov"), "{c}");
        assert!(c.contains("-map 0:V:0 -map 0:a? -vf scale=-2:720,format=nv12,hwupload -c:v hevc_vaapi -global_quality 32 -c:a copy"), "{c}");
        assert_eq!(p.uscita, PathBuf::from("/video/Znew_gara-720p-HEVC-AMD.mp4"));
        assert_eq!(p.temporaneo, PathBuf::from("/video/tmp_Znew_gara-720p-HEVC-AMD.mp4"));
        assert_eq!(p.risoluzione, 720);
        assert!(c.contains("-map_chapters -1 -movflags +faststart"), "{c}");
    }

    #[test]
    fn verticale_e_fps() {
        let o = Opzioni { fps_max: Some(30), separatore: "_".into(), ..Opzioni::default() };
        let p = pianifica(&video(1080, 1920, None), &[], &o, &backend("libx265", "CPU")).unwrap();
        let c = cmd(&p);
        assert!(c.contains("-vf fps=30,scale=720:-2,format=yuv420p -c:v libx265"), "{c}");
        assert!(!c.contains("vaapi_device"));
        assert!(p.uscita.ends_with("Znew_gara_720p_HEVC_CPU.mp4"));
        assert!(p.note.iter().any(|n| n.contains("60 → 30 fps")));
    }

    #[test]
    fn ritaglio_ai_multipli() {
        let mut v = video(1920, 802, Some("aac"));
        let o = Opzioni { risoluzione: None, ..Opzioni::default() };
        // HEVC: 802 non è multiplo di 8 → 800 (e 800 va bene anche per l'allineamento a 16)
        let p = pianifica(&v, &[], &o, &backend("hevc_vaapi", "AMD")).unwrap();
        assert!(cmd(&p).contains("-vf crop='min(iw,1920)':'min(ih,800)',format=nv12,hwupload"), "{}", cmd(&p));
        assert_eq!((p.previste, p.risoluzione), ((1920, 800), 800));
        assert!(p.uscita.ends_with("Znew_gara-800p-HEVC-AMD.mp4"));
        assert_eq!(p.note.len(), 1, "{:?}", p.note);
        // ridotto a 720p la larghezza diventa 1724: multiplo di 8 → 1720, allineamento a 16 → 1712
        let p = pianifica(&v, &[], &Opzioni::default(), &backend("hevc_vaapi", "AMD")).unwrap();
        assert!(cmd(&p).contains("scale=-2:720,crop='min(iw,1712)':'min(ih,720)'"), "{}", cmd(&p));
        assert!(p.note.iter().any(|n| n.contains("1724×720 ritagliato a 1720×720")), "{:?}", p.note);
        assert!(p.note.iter().any(|n| n.contains("1720×720 ritagliato a 1712×720")), "{:?}", p.note);
        // già allineato: nessun ritaglio
        v = video(1920, 800, Some("aac"));
        let p = pianifica(&v, &[], &o, &backend("hevc_vaapi", "AMD")).unwrap();
        assert!(!cmd(&p).contains("crop"));
        // anche sulla CPU l'HEVC va a multipli di 8: il problema è dei lettori, non dell'encoder
        let p = pianifica(&video(1920, 802, None), &[], &o, &backend("libx265", "CPU")).unwrap();
        assert!(cmd(&p).contains("crop='min(iw,1920)':'min(ih,800)'"), "{}", cmd(&p));
        // H.264: nessun ritaglio, nemmeno con dimensioni non multiple di 8
        let p = pianifica(&video(854, 480, None), &[], &o, &backend("libx264", "CPU")).unwrap();
        assert!(!cmd(&p).contains("crop"), "{}", cmd(&p));
        assert_eq!(p.previste, (854, 480));
        // misurato: l'encoder rispetta le dimensioni → nessun ritaglio
        let mut b = backend("hevc_vaapi", "AMD");
        b.allineamento = (1, 1);
        let p = pianifica(&video(1920, 1080, None), &[], &o, &b).unwrap();
        assert!(!cmd(&p).contains("crop"));
        assert_eq!(p.risoluzione, 1080);
        // misurato: l'encoder arrotonda l'altezza a 32 → 1080 diventa 1056
        b.allineamento = (64, 32);
        let p = pianifica(&video(1920, 1080, None), &[], &o, &b).unwrap();
        assert!(cmd(&p).contains("crop='min(iw,1920)':'min(ih,1056)'"), "{}", cmd(&p));
        assert_eq!(p.risoluzione, 1056);
        assert!(p.note[0].contains("1920×1056") && p.note[0].contains("64×32"), "{:?}", p.note);
    }

    #[test]
    fn candidati() {
        let c = candidati_ritaglio((854, 480));
        assert_eq!(c[0], (854, 480));
        assert_eq!(c[1], (848, 480)); // multiplo di 8/16 in larghezza: si perde meno
        assert!(c.contains(&(832, 480)) && c.contains(&(768, 480)));
        assert!(c.windows(2).all(|w| w[0].0 * w[0].1 >= w[1].0 * w[1].1));
        // già allineato a 64: le prime due prove sono l'originale e il multiplo di 128 in altezza
        assert_eq!(candidati_ritaglio((1920, 1088))[..2], [(1920, 1088), (1920, 1024)]);
    }

    #[test]
    fn ritaglio_da_verifica() {
        let o = Opzioni { risoluzione: None, ..Opzioni::default() };
        let mut b = backend("hevc_vaapi", "AMD");
        b.allineamento = (1, 1); // il rilevamento non aveva visto problemi
        // il caso reale: 854×480 → prima 848 (multiplo di 8), che l'encoder poi rispetta
        let v = video(854, 480, Some("aac"));
        let ok = Verifica { originali: (848, 480), prodotte: (848, 480), finali: (848, 480) };
        let p = pianifica_con(&v, &[], &o, &b, Some(ok)).unwrap();
        assert!(cmd(&p).contains("-vf crop='min(iw,848)':'min(ih,480)',format=nv12,hwupload"), "{}", cmd(&p));
        assert_eq!((p.previste, p.risoluzione), ((848, 480), 480));
        assert_eq!(p.note.len(), 1, "{:?}", p.note);
        assert!(p.note[0].contains("854×480 ritagliato a 848×480"), "{:?}", p.note);
        // l'encoder altera anche 848: si usa il ritaglio trovato dalla prova
        let vf = Verifica { originali: (848, 480), prodotte: (894, 480), finali: (832, 480) };
        let p = pianifica_con(&v, &[], &o, &b, Some(vf)).unwrap();
        assert!(cmd(&p).contains("-vf crop='min(iw,832)':'min(ih,480)'"), "{}", cmd(&p));
        assert!(p.note.iter().any(|n| n.contains("894×480") && n.contains("832×480")), "{:?}", p.note);
        // la verifica dice che va bene così: nessun ritaglio, anche se il rilevamento ne chiedeva uno
        b.allineamento = (16, 16);
        let v = video(1288, 480, None);
        let ok = Verifica { originali: (1288, 480), prodotte: (1288, 480), finali: (1288, 480) };
        let p = pianifica_con(&v, &[], &o, &b, Some(ok)).unwrap();
        assert!(!cmd(&p).contains("crop"), "{}", cmd(&p));
        // verifica di altre dimensioni (non dovrebbe succedere): vale la regola del rilevamento
        let altra = Verifica { originali: (1280, 720), prodotte: (1280, 720), finali: (1280, 720) };
        let p = pianifica_con(&v, &[], &o, &b, Some(altra)).unwrap();
        assert!(cmd(&p).contains("crop='min(iw,1280)':'min(ih,480)'"), "{}", cmd(&p));
        // i software non si provano
        assert!(verifica_dimensioni("/inesistente", &backend("libx264", "CPU"), (854, 480)).is_none());
    }

    #[test]
    fn niente_ingrandimenti() {
        let p = pianifica(&video(854, 480, None), &[], &Opzioni::default(), &backend("h264_nvenc", "NVENC")).unwrap();
        assert!(cmd(&p).contains("-vf format=yuv420p -c:v h264_nvenc -preset p5"));
        assert_eq!(p.risoluzione, 480);
        assert!(p.uscita.ends_with("Znew_gara-480p-H264-NVENC.mp4"));
    }

    #[test]
    fn audio_non_adatto() {
        let p = pianifica(&video(1280, 720, Some("pcm_s16le")), &[], &Opzioni::default(), &backend("libx264", "CPU")).unwrap();
        assert!(cmd(&p).contains("-c:a aac -b:a 128k"));
        assert!(p.note[0].contains("pcm_s16le"));
    }

    #[test]
    fn con_tagli() {
        let o = Opzioni { cartella: Some("/uscita".into()), ..Opzioni::default() };
        let p = pianifica(&video(1920, 1080, Some("aac")), &[(10.0, 20.0)], &o, &backend("hevc_vaapi", "AMD")).unwrap();
        let c = cmd(&p);
        assert!(c.contains("-filter_complex [0:v:0]split=2"), "{c}");
        assert!(c.contains("[vc]scale=-2:720,format=nv12,hwupload[vo] -map [vo] -map [ac]"), "{c}");
        assert!(c.contains("-c:a aac"));
        assert!((p.durata - 50.0).abs() < 1e-9);
        assert!(p.uscita.starts_with("/uscita"));
        assert!(pianifica(&video(1920, 1080, None), &[(0.0, 60.0)], &o, &backend("libx264", "CPU")).is_err());
    }

    #[test]
    fn lettura_sonda() {
        let json = r#"{"streams":[
            {"codec_type":"video","codec_name":"mjpeg","width":300,"height":300,"disposition":{"attached_pic":1}},
            {"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"30000/1001",
             "r_frame_rate":"30/1","disposition":{"attached_pic":0},"side_data_list":[{"rotation":-90}]},
            {"codec_type":"audio","codec_name":"aac","disposition":{"attached_pic":0}}],
            "format":{"duration":"12.500000"}}"#;
        let v = analizza_sonda(json, Path::new("/x.mp4"), 5).unwrap();
        assert_eq!((v.larghezza, v.altezza), (1080, 1920));
        assert!(v.verticale() && v.lato_corto() == 1080);
        assert!((v.fps - 29.97).abs() < 0.01);
        assert_eq!(v.durata, 12.5);
        assert_eq!(v.audio.as_deref(), Some("aac"));
        assert!(analizza_sonda(r#"{"streams":[]}"#, Path::new("/x"), 0).is_err());
    }

    #[test]
    fn dimensioni() {
        assert_eq!(umano(512), "512 B");
        assert_eq!(umano(1536), "1,5 KB");
        assert_eq!(umano(3 * 1024 * 1024 * 1024), "3,0 GB");
    }
}
