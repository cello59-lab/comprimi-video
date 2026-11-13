//! Gli encoder conosciuti e come vanno chiamati: dispositivo, filtro finale e qualità.
//! Questo modulo è lo «strato di traduzione» che userà anche il nucleo di compressione.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Codec {
    pub const TUTTI: [Codec; 3] = [Codec::H264, Codec::Hevc, Codec::Av1];

    pub fn nome(self) -> &'static str {
        match self {
            Codec::H264 => "H.264",
            Codec::Hevc => "HEVC",
            Codec::Av1 => "AV1",
        }
    }

    /// Nome nel suffisso dei file (come nella versione Python).
    pub fn sigla(self) -> &'static str {
        match self {
            Codec::H264 => "H264",
            Codec::Hevc => "HEVC",
            Codec::Av1 => "AV1",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Famiglia {
    Nvenc,
    Vaapi,
    Software,
}

impl Famiglia {
    pub fn nome(self) -> &'static str {
        match self {
            Famiglia::Nvenc => "NVENC",
            Famiglia::Vaapi => "VAAPI",
            // tradotto da chi lo mostra
            Famiglia::Software => crate::i18n::da_tradurre("software (CPU)"),
        }
    }
}

#[derive(Debug)]
pub struct DefEncoder {
    pub nome: &'static str,
    pub codec: Codec,
    pub famiglia: Famiglia,
    /// A parità di famiglia e codec, il valore più basso è preferito.
    pub preferenza: u8,
}

pub const ENCODER: &[DefEncoder] = &[
    DefEncoder { nome: "h264_nvenc", codec: Codec::H264, famiglia: Famiglia::Nvenc, preferenza: 0 },
    DefEncoder { nome: "hevc_nvenc", codec: Codec::Hevc, famiglia: Famiglia::Nvenc, preferenza: 0 },
    DefEncoder { nome: "av1_nvenc", codec: Codec::Av1, famiglia: Famiglia::Nvenc, preferenza: 0 },
    DefEncoder { nome: "h264_vaapi", codec: Codec::H264, famiglia: Famiglia::Vaapi, preferenza: 0 },
    DefEncoder { nome: "hevc_vaapi", codec: Codec::Hevc, famiglia: Famiglia::Vaapi, preferenza: 0 },
    DefEncoder { nome: "av1_vaapi", codec: Codec::Av1, famiglia: Famiglia::Vaapi, preferenza: 0 },
    DefEncoder { nome: "libx264", codec: Codec::H264, famiglia: Famiglia::Software, preferenza: 0 },
    DefEncoder { nome: "libx265", codec: Codec::Hevc, famiglia: Famiglia::Software, preferenza: 0 },
    DefEncoder { nome: "libsvtav1", codec: Codec::Av1, famiglia: Famiglia::Software, preferenza: 0 },
    // molto più lento di SVT-AV1: solo se quello manca
    DefEncoder { nome: "libaom-av1", codec: Codec::Av1, famiglia: Famiglia::Software, preferenza: 1 },
];

pub fn cerca(nome: &str) -> Option<&'static DefEncoder> {
    ENCODER.iter().find(|d| d.nome == nome)
}

fn scala(q: u8, massimo: u32) -> String {
    let v = (q as u32 * massimo + 25) / 51;
    v.clamp(1, massimo).to_string()
}

impl DefEncoder {
    /// Opzioni da mettere PRIMA di `-i` (scelta del dispositivo).
    pub fn argomenti_dispositivo(&self, nodo: Option<&str>) -> Vec<String> {
        match (self.famiglia, nodo) {
            (Famiglia::Vaapi, Some(n)) => vec!["-vaapi_device".into(), n.into()],
            _ => vec![],
        }
    }

    /// Coda della catena di filtri video (va in fondo, dopo scale/fps).
    pub fn filtro_finale(&self) -> &'static str {
        match self.famiglia {
            Famiglia::Vaapi => "format=nv12,hwupload",
            // 8 bit 4:2:0: il formato che tutti i lettori digeriscono
            _ => "format=yuv420p",
        }
    }

    /// Opzioni di qualità. `q` è il valore del cursore dell'app (scala 0–51,
    /// più basso = qualità migliore), tradotto nella scala dell'encoder.
    /// NB: l'equivalenza fra le scale va tarata con prove reali nella fase del nucleo.
    pub fn argomenti_qualita(&self, q: u8) -> Vec<String> {
        let q51 = q.min(51).to_string();
        let v: Vec<&str> = match (self.famiglia, self.nome) {
            (Famiglia::Vaapi, "av1_vaapi") => return vec!["-global_quality".into(), scala(q, 255)],
            (Famiglia::Vaapi, _) => return vec!["-global_quality".into(), q51],
            (Famiglia::Nvenc, _) => {
                return ["-preset", "p5", "-rc", "vbr", "-cq", &q51, "-b:v", "0"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            }
            (_, "libx264") => vec!["-preset", "medium", "-crf"],
            (_, "libx265") => vec!["-preset", "medium", "-x265-params", "log-level=error", "-crf"],
            (_, "libsvtav1") => return vec!["-preset".into(), "8".into(), "-crf".into(), scala(q, 63)],
            (_, "libaom-av1") => {
                return vec![
                    "-cpu-used".into(),
                    "6".into(),
                    "-row-mt".into(),
                    "1".into(),
                    "-b:v".into(),
                    "0".into(),
                    "-crf".into(),
                    scala(q, 63),
                ]
            }
            _ => vec![],
        };
        let mut out: Vec<String> = v.into_iter().map(String::from).collect();
        out.push(q51);
        out
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn qualita() {
        assert_eq!(cerca("hevc_vaapi").unwrap().argomenti_qualita(32), ["-global_quality", "32"]);
        assert_eq!(cerca("av1_vaapi").unwrap().argomenti_qualita(32), ["-global_quality", "160"]);
        assert_eq!(cerca("av1_vaapi").unwrap().argomenti_qualita(0), ["-global_quality", "1"]);
        assert_eq!(
            cerca("hevc_nvenc").unwrap().argomenti_qualita(30),
            ["-preset", "p5", "-rc", "vbr", "-cq", "30", "-b:v", "0"]
        );
        assert_eq!(cerca("libx264").unwrap().argomenti_qualita(23), ["-preset", "medium", "-crf", "23"]);
        assert_eq!(
            cerca("libsvtav1").unwrap().argomenti_qualita(51),
            ["-preset", "8", "-crf", "63"]
        );
    }

    #[test]
    fn dispositivo_e_filtro() {
        let v = cerca("h264_vaapi").unwrap();
        assert_eq!(v.argomenti_dispositivo(Some("/dev/dri/renderD128")), ["-vaapi_device", "/dev/dri/renderD128"]);
        assert_eq!(v.filtro_finale(), "format=nv12,hwupload");
        let s = cerca("libx265").unwrap();
        assert!(s.argomenti_dispositivo(Some("/dev/dri/renderD128")).is_empty());
        assert_eq!(s.filtro_finale(), "format=yuv420p");
    }

    #[test]
    fn ogni_famiglia_copre_tutti_i_codec() {
        for f in [Famiglia::Nvenc, Famiglia::Vaapi, Famiglia::Software] {
            for c in Codec::TUTTI {
                assert!(ENCODER.iter().any(|d| d.famiglia == f && d.codec == c), "{f:?} {c:?}");
            }
        }
    }
}
