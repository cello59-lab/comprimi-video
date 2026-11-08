//! Il nucleo: sonda del video, piano del comando ffmpeg ed esecuzione con avanzamento.
//! Nessuna dipendenza dall'interfaccia: lo userà sia la riga di comando sia la finestra GTK.

use crate::encoder::{DefEncoder, Famiglia};
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
        .map_err(|e| format!("impossibile avviare ffprobe: {e}"))?;
    let dimensione = std::fs::metadata(percorso).map(|m| m.len()).unwrap_or(0);
    analizza_sonda(&String::from_utf8_lossy(&out.stdout), percorso, dimensione)
}

pub fn analizza_sonda(json: &str, percorso: &Path, dimensione: u64) -> Result<InfoVideo, String> {
    let d: serde_json::Value = serde_json::from_str(json).map_err(|_| "file non leggibile".to_string())?;
    let flussi = d["streams"].as_array().cloned().unwrap_or_default();
    let video = flussi
        .iter()
        .find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_i64().unwrap_or(0) == 0)
        .ok_or("nessun flusso video trovato")?;
    let (w, h) = (video["width"].as_u64().unwrap_or(0) as u32, video["height"].as_u64().unwrap_or(0) as u32);
    if w == 0 || h == 0 {
        return Err("risoluzione non rilevata".into());
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
}

impl Backend {
    pub fn da_prova(p: &Prova, r: &Rilevamento) -> Option<Backend> {
        let encoder = crate::encoder::cerca(&p.encoder)?;
        let gpu = p.gpu.and_then(|i| r.gpu.get(i));
        let sigla = match encoder.famiglia {
            Famiglia::Vaapi => gpu.map_or("GPU", |g| g.produttore.sigla()).to_string(),
            Famiglia::Nvenc => "NVENC".into(),
            Famiglia::Software => "CPU".into(),
        };
        let descrizione = match gpu {
            Some(g) => format!("{} su {}", encoder.nome, g.nome),
            None => format!("{} sulla CPU", encoder.nome),
        };
        Some(Backend {
            encoder,
            nodo: if encoder.famiglia == Famiglia::Vaapi { gpu.and_then(|g| g.nodo.clone()) } else { None },
            sigla,
            descrizione,
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
    // Gli encoder VAAPI (in particolare AMD) lavorano a blocchi di 16 pixel: con dimensioni
    // non multiple riempiono il resto con dati non validi (banda verde, es. 1920×802 → 1920×810).
    // Si ritaglia al multiplo di 16 inferiore, al centro: si perdono al massimo 15 pixel
    // per lato, di solito dalle bande nere dei film.
    if b.encoder.famiglia == Famiglia::Vaapi && (lw % 16 != 0 || lh % 16 != 0) {
        filtri.push("crop=trunc(iw/16)*16:trunc(ih/16)*16".into());
        note.push(format!(
            "{lw}×{lh} non è multiplo di 16: ritagliato a {}×{} per l'encoder VAAPI",
            lw / 16 * 16,
            lh / 16 * 16
        ));
        risoluzione = risoluzione / 16 * 16;
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
            note.push(format!("audio {c} non adatto a MP4: ricodificato in AAC"));
        }
    }

    let mut durata = v.durata;
    if tagli.is_empty() {
        a.extend(["-map", "0:V:0", "-map", "0:a?", "-vf"].iter().map(|s| s.to_string()));
        a.push(filtro);
    } else {
        let tenute = tempo::parti_tenute(tagli, v.durata);
        if tenute.is_empty() {
            return Err("i tagli eliminano tutto il video".into());
        }
        let con_audio = v.audio.is_some();
        durata = tempo::durata_dopo_tagli(tagli, v.durata);
        let n = tempo::unisci_tagli(tagli).len();
        note.push(format!(
            "✂ {n} {} − {}, durata finale {}",
            if n == 1 { "parte eliminata" } else { "parti eliminate" },
            tempo::fmt_t(tempo::durata_tagliata(tagli, v.durata)),
            tempo::fmt_t(durata)
        ));
        a.push("-filter_complex".into());
        a.push(tempo::filtro_con_tagli(&tenute, con_audio, &filtro));
        a.extend(["-map", "[vo]"].iter().map(|s| s.to_string()));
        if con_audio {
            a.extend(["-map", "[ac]"].iter().map(|s| s.to_string()));
            if !ricodifica {
                ricodifica = true; // l'audio tagliato non si può copiare
                note.push("con i tagli l'audio viene ricodificato in AAC".into());
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

    Ok(Piano { argomenti: a, uscita, temporaneo, risoluzione, durata, note })
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
        Err(e) => return Fine::Errore(vec![format!("impossibile avviare ffmpeg: {e}")]),
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
            Err(e) => Fine::Errore(vec![format!("impossibile rinominare il file: {e}")]),
        },
        _ => {
            let _ = std::fs::remove_file(&piano.temporaneo);
            let u = ultime.lock().unwrap();
            Fine::Errore(if u.is_empty() { vec!["ffmpeg è terminato con un errore".into()] } else { u.iter().cloned().collect() })
        }
    }
}

pub fn umano(n: u64) -> String {
    let mut v = n as f64;
    for unita in ["B", "KB", "MB", "GB", "TB"] {
        if v < 1024.0 {
            return if unita == "B" { format!("{v:.0} B") } else { format!("{v:.1} {unita}").replace('.', ",") };
        }
        v /= 1024.0;
    }
    format!("{v:.1} PB").replace('.', ",")
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
    fn vaapi_multiplo_di_16() {
        let mut v = video(1920, 802, Some("aac"));
        let o = Opzioni { risoluzione: None, ..Opzioni::default() };
        let p = pianifica(&v, &[], &o, &backend("hevc_vaapi", "AMD")).unwrap();
        assert!(cmd(&p).contains("-vf crop=trunc(iw/16)*16:trunc(ih/16)*16,format=nv12,hwupload"), "{}", cmd(&p));
        assert_eq!(p.risoluzione, 800);
        assert!(p.uscita.ends_with("Znew_gara-800p-HEVC-AMD.mp4"));
        // ridotto a 720p la larghezza diventa 1724: anche lei va ritagliata
        let p = pianifica(&v, &[], &Opzioni::default(), &backend("hevc_vaapi", "AMD")).unwrap();
        assert!(cmd(&p).contains("scale=-2:720,crop="), "{}", cmd(&p));
        assert!(p.note.iter().any(|n| n.contains("1724×720") && n.contains("1712×720")), "{:?}", p.note);
        // già allineato: nessun ritaglio
        v = video(1920, 800, Some("aac"));
        let p = pianifica(&v, &[], &o, &backend("hevc_vaapi", "AMD")).unwrap();
        assert!(!cmd(&p).contains("crop"));
        // gli encoder software gestiscono da soli le dimensioni: nessun ritaglio
        let p = pianifica(&video(1920, 802, None), &[], &o, &backend("libx265", "CPU")).unwrap();
        assert!(!cmd(&p).contains("crop"));
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
