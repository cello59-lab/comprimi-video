//! Prove di codifica reali: per ogni scheda e ogni encoder compilato in ffmpeg si codificano
//! pochi fotogrammi di prova. Solo così si sa cosa funziona davvero su questa macchina.

use crate::cache;
use crate::encoder::{Codec, DefEncoder, Famiglia, ENCODER};
use crate::ffmpeg::{self, InfoFfmpeg};
use crate::gpu::{self, Gpu, Produttore};
use serde::{Deserialize, Serialize};
use std::io::{IsTerminal, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Da aumentare quando cambia il formato o la logica delle prove (invalida la cache).
pub const FORMATO: u32 = 3;
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "stato", content = "motivo")]
pub enum Esito {
    Funziona,
    NonFunziona(String),
    /// L'encoder non è compilato in questo ffmpeg
    NonIncluso,
    /// Non provato per un motivo noto in anticipo
    NonProvato(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prova {
    pub encoder: String,
    pub codec: Codec,
    pub famiglia: Famiglia,
    /// Indice in `Rilevamento::gpu` (None = software)
    pub gpu: Option<usize>,
    pub esito: Esito,
    pub durata_ms: u64,
    /// Multipli (larghezza, altezza) a cui l'encoder porta le dimensioni senza dirlo al lettore
    /// (le righe in più diventano una banda verde): (1, 1) = dimensioni rispettate.
    /// Misurato solo per gli encoder su GPU; None = non misurato.
    #[serde(default)]
    pub allineamento: Option<(u32, u32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rilevamento {
    pub formato: u32,
    pub impronta: String,
    /// Secondi dal 1970
    pub creato: u64,
    #[serde(default)]
    pub dalla_cache: bool,
    pub ffmpeg: Option<InfoFfmpeg>,
    pub gpu: Vec<Gpu>,
    pub prove: Vec<Prova>,
}

impl Rilevamento {
    /// L'encoder consigliato per un codec: hardware prima del software.
    pub fn consigliato(&self, codec: Codec) -> Option<&Prova> {
        self.prove
            .iter()
            .filter(|p| p.codec == codec && p.esito == Esito::Funziona)
            .min_by_key(|p| rango(p, &self.gpu))
    }
}

fn rango(p: &Prova, gpu: &[Gpu]) -> (u8, u8, u8, usize) {
    let pref = crate::encoder::cerca(&p.encoder).map_or(9, |d| d.preferenza);
    let prod = p.gpu.and_then(|i| gpu.get(i)).map(|g| g.produttore);
    let fam = match p.famiglia {
        Famiglia::Nvenc => 0,
        Famiglia::Vaapi => 1,
        Famiglia::Software => 9,
    };
    // fra le VAAPI: AMD, poi Intel (spesso integrata), poi altre
    let marca = match prod {
        Some(Produttore::Amd) => 0,
        Some(Produttore::Intel) => 1,
        _ => 2,
    };
    (fam, marca, pref, p.gpu.unwrap_or(usize::MAX))
}

fn adesso() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Usa la cache se è ancora valida per questa macchina, altrimenti rifà le prove.
pub fn rileva_con_cache(forza: bool, avanzamento: bool) -> Rilevamento {
    let gpu = gpu::elenca();
    let ff = ffmpeg::trova();
    let impronta = cache::impronta(ff.as_ref(), &gpu);
    if !forza {
        if let Some(mut r) = cache::leggi(&impronta) {
            r.dalla_cache = true;
            // il percorso attuale di ffmpeg (nell'AppImage cambia a ogni avvio) e le schede
            // con i nomi di adesso; le prove restano quelle salvate
            r.ffmpeg = ff;
            r.gpu = gpu;
            return r;
        }
    }
    let r = rileva(ff, gpu, impronta, avanzamento);
    if r.ffmpeg.is_some() {
        cache::scrivi(&r);
    }
    r
}

pub fn rileva(ff: Option<InfoFfmpeg>, gpu: Vec<Gpu>, impronta: String, avanzamento: bool) -> Rilevamento {
    let mostra = avanzamento && std::io::stderr().is_terminal();
    let mut prove = Vec::new();

    if let Some(ff) = &ff {
        for (i, g) in gpu.iter().enumerate() {
            let famiglia = if g.produttore == Produttore::Nvidia { Famiglia::Nvenc } else { Famiglia::Vaapi };
            // NVENC si prova una volta sola anche con più schede NVIDIA
            if famiglia == Famiglia::Nvenc && gpu[..i].iter().any(|h| h.produttore == Produttore::Nvidia) {
                continue;
            }
            for d in ENCODER.iter().filter(|d| d.famiglia == famiglia) {
                let ostacolo = ostacolo(ff, d, g);
                prove.push(esegui_o_salta(ff, d, Some(i), g.nodo.as_deref(), ostacolo, mostra));
            }
        }
        for d in ENCODER.iter().filter(|d| d.famiglia == Famiglia::Software) {
            let ostacolo = (!ff.ha_encoder(d.nome)).then_some(Esito::NonIncluso);
            prove.push(esegui_o_salta(ff, d, None, None, ostacolo, mostra));
        }
    }
    if mostra {
        eprint!("\r\x1b[K");
    }

    Rilevamento {
        formato: FORMATO,
        impronta,
        creato: adesso(),
        dalla_cache: false,
        ffmpeg: ff,
        gpu,
        prove,
    }
}

/// Motivi noti per cui una prova è inutile.
fn ostacolo(ff: &InfoFfmpeg, d: &DefEncoder, g: &Gpu) -> Option<Esito> {
    if !ff.ha_encoder(d.nome) {
        return Some(Esito::NonIncluso);
    }
    match d.famiglia {
        Famiglia::Vaapi => match &g.nodo {
            None => Some(Esito::NonProvato("la scheda non ha un dispositivo /dev/dri/renderD*".into())),
            Some(n) if !g.accessibile => Some(Esito::NonProvato(format!(
                "permesso negato su {n}: l'utente deve far parte del gruppo «render»"
            ))),
            _ => None,
        },
        Famiglia::Nvenc if g.driver.as_deref() == Some("nouveau") => Some(Esito::NonProvato(
            "driver libero nouveau: per NVENC serve il driver proprietario NVIDIA".into(),
        )),
        _ => None,
    }
}

fn esegui_o_salta(
    ff: &InfoFfmpeg,
    d: &DefEncoder,
    gpu: Option<usize>,
    nodo: Option<&str>,
    ostacolo: Option<Esito>,
    mostra: bool,
) -> Prova {
    let (esito, durata_ms, allineamento) = match ostacolo {
        Some(e) => (e, 0, None),
        None => {
            if mostra {
                eprint!("\r\x1b[KProvo {}…", d.nome);
            }
            esegui_prova(&ff.percorso, d, nodo)
        }
    };
    Prova {
        encoder: d.nome.into(),
        codec: d.codec,
        famiglia: d.famiglia,
        gpu,
        esito,
        durata_ms,
        allineamento,
    }
}

/// Dimensioni della prova: pari ma non multiple di 16, 32, 64 o 128, e scelte in modo che
/// l'arrotondamento a ciascuno di questi multipli dia un risultato diverso.
pub const PROVA_L: u32 = 1282;
pub const PROVA_A: u32 = 642;

/// Argomenti della codifica di prova: 15 fotogrammi generati da ffmpeg, scritti in `uscita`
/// (per misurarne le dimensioni) oppure buttati via.
pub fn comando_prova(d: &DefEncoder, nodo: Option<&str>, uscita: Option<&Path>) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    a.extend(d.argomenti_dispositivo(nodo));
    a.push("-f".into());
    a.push("lavfi".into());
    a.push("-i".into());
    a.push(format!("testsrc2=size={PROVA_L}x{PROVA_A}:rate=30"));
    for s in ["-frames:v", "15", "-an", "-vf"] {
        a.push(s.into());
    }
    a.push(d.filtro_finale().into());
    a.push("-c:v".into());
    a.push(d.nome.into());
    a.extend(d.argomenti_qualita(30));
    match uscita {
        Some(p) => {
            a.push("-f".into());
            a.push("matroska".into());
            a.push(p.to_string_lossy().into_owned());
        }
        None => a.extend(["-f", "null", "-"].iter().map(|s| s.to_string())),
    }
    a
}

/// Multiplo a cui è stata arrotondata una dimensione: 1 se è rimasta uguale,
/// None se il risultato non corrisponde a nessun arrotondamento noto.
pub fn multiplo(prima: u32, dopo: u32) -> Option<u32> {
    if dopo == prima {
        return Some(1);
    }
    [2, 4, 8, 16, 32, 64, 128].into_iter().find(|a| prima.div_ceil(*a) * a == dopo)
}

/// Larghezza e altezza del video in un file, lette con ffprobe.
fn dimensioni(ffmpeg: &str, file: &Path) -> Option<(u32, u32)> {
    let out = Command::new(crate::lavoro::ffprobe_per(ffmpeg))
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0"])
        .arg(file)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let t = String::from_utf8_lossy(&out.stdout);
    let (w, h) = t.trim().split_once(',')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

fn esegui_prova(ffmpeg: &str, d: &DefEncoder, nodo: Option<&str>) -> (Esito, u64, Option<(u32, u32)>) {
    // per gli encoder su GPU il risultato si scrive su file, per misurarne le dimensioni
    let file = (d.famiglia != Famiglia::Software).then(|| {
        std::env::temp_dir().join(format!("comprimi-video-prova-{}-{}.mkv", std::process::id(), d.nome))
    });
    let (esito, ms) = esegui_comando(ffmpeg, comando_prova(d, nodo, file.as_deref()));
    let mut allineamento = None;
    if let Some(f) = &file {
        if esito == Esito::Funziona {
            allineamento = Some(match dimensioni(ffmpeg, f) {
                // dimensioni sconosciute o arrotondate in modo strano: si sta larghi
                Some((w, h)) => (multiplo(PROVA_L, w).unwrap_or(64), multiplo(PROVA_A, h).unwrap_or(64)),
                None => (16, 16),
            });
        }
        let _ = std::fs::remove_file(f);
    }
    (esito, ms, allineamento)
}

fn esegui_comando(ffmpeg: &str, argomenti: Vec<String>) -> (Esito, u64) {
    let inizio = Instant::now();
    let mut figlio = match Command::new(ffmpeg)
        .args(argomenti)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(f) => f,
        Err(e) => return (Esito::NonFunziona(format!("impossibile avviare ffmpeg: {e}")), 0),
    };
    // stderr letto in un thread a parte, così ffmpeg non si blocca a buffer pieno
    let mut err = figlio.stderr.take().expect("stderr");
    let lettore = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let stato = loop {
        match figlio.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if inizio.elapsed() > TIMEOUT => {
                let _ = figlio.kill();
                let _ = figlio.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(_) => break None,
        }
    };
    let stderr = lettore.join().unwrap_or_default();
    let ms = inizio.elapsed().as_millis() as u64;
    let esito = match stato {
        Some(s) if s.success() => Esito::Funziona,
        Some(_) => Esito::NonFunziona(spiega(&stderr)),
        None => Esito::NonFunziona(format!("nessuna risposta entro {} s", TIMEOUT.as_secs())),
    };
    (esito, ms)
}

/// Traduce gli errori di ffmpeg più comuni in una spiegazione comprensibile.
pub fn spiega(stderr: &str) -> String {
    let t = stderr.to_lowercase();
    let c = |s: &str| t.contains(s);
    if c("no usable encoding profile") || c("no usable encoding entrypoint") || c("entrypoint not supported") {
        return "la scheda non supporta la codifica in questo formato".into();
    }
    if c("required nvenc api version") || c("driver does not support") {
        return "driver NVIDIA troppo vecchio per questo ffmpeg".into();
    }
    if c("cannot load libcuda") || c("cannot load libnvidia-encode") {
        return "librerie NVIDIA assenti: serve il driver proprietario NVIDIA".into();
    }
    if c("unsupported device") || c("no capable devices found") || c("openencodesessionex failed") {
        return "la scheda NVIDIA non supporta questo formato".into();
    }
    if c("failed to initialise vaapi") || c("vainitialize failed") || c("va_openDriver".to_lowercase().as_str()) {
        return "driver VAAPI mancante o non funzionante (es. mesa-va-drivers, intel-media-va-driver)".into();
    }
    if c("permission denied") {
        return "permesso negato sul dispositivo (gruppo «render»)".into();
    }
    let ultima = stderr.lines().rev().map(str::trim).find(|r| !r.is_empty()).unwrap_or("errore sconosciuto");
    let mut s: String = ultima.chars().take(160).collect();
    if ultima.chars().count() > 160 {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::encoder::cerca;

    #[test]
    fn spiegazioni() {
        assert_eq!(
            spiega("[av1_vaapi @ 0x5] No usable encoding entrypoint found for profile VAProfileAV1Profile0 (32).\nError"),
            "la scheda non supporta la codifica in questo formato"
        );
        assert!(spiega("[hevc_nvenc @ 0x1] Cannot load libcuda.so.1").contains("driver proprietario"));
        assert!(spiega("Failed to initialise VAAPI connection: -1 (unknown libva error).").contains("VAAPI"));
        assert_eq!(spiega("\n  qualcosa di strano  \n\n"), "qualcosa di strano");
    }

    #[test]
    fn comando() {
        let a = comando_prova(cerca("hevc_vaapi").unwrap(), Some("/dev/dri/renderD128"), Some(Path::new("/tmp/p.mkv")));
        let s = a.join(" ");
        assert!(s.contains("-vaapi_device /dev/dri/renderD128 -f lavfi -i testsrc2=size=1282x642"), "{s}");
        assert!(s.contains("-vf format=nv12,hwupload -c:v hevc_vaapi -global_quality 30 -f matroska /tmp/p.mkv"), "{s}");
        let a = comando_prova(cerca("libx264").unwrap(), None, None);
        assert!(a.join(" ").ends_with("-f null -"));
    }

    #[test]
    fn multipli() {
        assert_eq!(multiplo(PROVA_L, PROVA_L), Some(1));
        assert_eq!(multiplo(PROVA_A, 656), Some(16));
        assert_eq!(multiplo(PROVA_A, 672), Some(32));
        assert_eq!(multiplo(PROVA_A, 704), Some(64));
        assert_eq!(multiplo(PROVA_A, 768), Some(128));
        assert_eq!(multiplo(PROVA_L, 1296), Some(16));
        assert_eq!(multiplo(PROVA_L, 1312), Some(32));
        assert_eq!(multiplo(PROVA_L, 1344), Some(64));
        assert_eq!(multiplo(PROVA_A, 650), None);
    }

    fn prova(enc: &str, gpu: Option<usize>) -> Prova {
        let d = cerca(enc).unwrap();
        Prova {
            encoder: enc.into(),
            codec: d.codec,
            famiglia: d.famiglia,
            gpu,
            esito: Esito::Funziona,
            durata_ms: 0,
            allineamento: None,
        }
    }

    fn scheda(p: Produttore) -> Gpu {
        Gpu {
            nome: String::new(),
            produttore: p,
            vendor_id: 0,
            device_id: 0,
            pci: None,
            nodo: None,
            driver: None,
            accessibile: true,
            principale: false,
        }
    }

    #[test]
    fn scelta_consigliata() {
        let mut r = Rilevamento {
            formato: FORMATO,
            impronta: String::new(),
            creato: 0,
            dalla_cache: false,
            ffmpeg: None,
            gpu: vec![scheda(Produttore::Intel), scheda(Produttore::Amd)],
            prove: vec![
                prova("libx265", None),
                prova("hevc_vaapi", Some(0)),
                prova("hevc_vaapi", Some(1)),
                prova("libaom-av1", None),
                prova("libsvtav1", None),
            ],
        };
        let h = r.consigliato(Codec::Hevc).unwrap();
        assert_eq!((h.encoder.as_str(), h.gpu), ("hevc_vaapi", Some(1))); // AMD prima di Intel
        assert_eq!(r.consigliato(Codec::Av1).unwrap().encoder, "libsvtav1");
        assert!(r.consigliato(Codec::H264).is_none());
        r.prove[2].esito = Esito::NonFunziona("x".into());
        assert_eq!(r.consigliato(Codec::Hevc).unwrap().gpu, Some(0));
    }
}
