//! Lingua dell'interfaccia: italiano o inglese.
//!
//! I testi nel codice sono in italiano e passano da `t()` (testi fissi) o `tf()` (testi con
//! valori: ogni `{}` viene sostituito, nell'ordine, dagli argomenti). In inglese vengono
//! cercati nella tabella in `traduzioni.rs`; un test controlla che ogni testo usato nel codice
//! abbia la sua traduzione.
//!
//! La lingua si sceglie nel menu dell'app (chiave «lingua» della configurazione:
//! «auto», «it» o «en»). Con «auto» si segue la lingua del sistema: italiano se è italiana,
//! inglese in tutti gli altri casi.

use std::collections::HashMap;
use std::fmt::Display;
use std::path::PathBuf;
#[cfg(not(test))]
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[path = "traduzioni.rs"]
mod traduzioni;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lingua {
    It,
    En,
}

/// Preferenze possibili, come vengono salvate nella configurazione.
pub const PREFERENZE: [&str; 3] = ["auto", "it", "en"];

// 0 = non ancora decisa, 1 = italiano, 2 = inglese. Nei test vale per il solo thread,
// perché i test girano in parallelo e alcuni cambiano lingua.
#[cfg(not(test))]
static ATTUALE: AtomicU8 = AtomicU8::new(0);
#[cfg(not(test))]
fn leggi_stato() -> u8 {
    ATTUALE.load(Ordering::Relaxed)
}
#[cfg(not(test))]
fn scrivi_stato(v: u8) {
    ATTUALE.store(v, Ordering::Relaxed)
}
#[cfg(test)]
thread_local! {
    static ATTUALE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
fn leggi_stato() -> u8 {
    ATTUALE.with(|a| a.get())
}
#[cfg(test)]
fn scrivi_stato(v: u8) {
    ATTUALE.with(|a| a.set(v))
}

pub fn percorso_config() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("comprimi-video/config.json")
}

/// La preferenza salvata: «auto» (predefinita), «it» o «en».
pub fn preferenza() -> String {
    std::fs::read_to_string(percorso_config())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("lingua").and_then(|l| l.as_str()).map(String::from))
        .filter(|p| PREFERENZE.contains(&p.as_str()))
        .unwrap_or_else(|| "auto".into())
}

/// Lingua del sistema: le variabili sono quelle di gettext, nello stesso ordine.
pub fn da_sistema() -> Lingua {
    let var = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
    let locale = var("LC_ALL").or_else(|| var("LC_MESSAGES")).or_else(|| var("LANG")).unwrap_or_default();
    if locale.is_empty() || locale == "C" || locale.starts_with("C.") || locale == "POSIX" {
        return Lingua::En;
    }
    // LANGUAGE (elenco di preferenze) vale solo se il locale non è «C»
    let prima = var("LANGUAGE")
        .and_then(|l| l.split(':').find(|s| !s.is_empty()).map(String::from))
        .unwrap_or(locale);
    if prima.starts_with("it") {
        Lingua::It
    } else {
        Lingua::En
    }
}

pub fn risolvi(preferenza: &str) -> Lingua {
    match preferenza {
        "it" => Lingua::It,
        "en" => Lingua::En,
        _ => da_sistema(),
    }
}

pub fn imposta(l: Lingua) {
    scrivi_stato(if l == Lingua::It { 1 } else { 2 });
}

pub fn lingua() -> Lingua {
    match leggi_stato() {
        1 => Lingua::It,
        2 => Lingua::En,
        // nei test i testi attesi sono quelli italiani
        _ if cfg!(test) => Lingua::It,
        _ => {
            let l = risolvi(&preferenza());
            imposta(l);
            l
        }
    }
}

/// Codice della lingua attuale, es. per la cache del rilevamento.
pub fn codice() -> &'static str {
    match lingua() {
        Lingua::It => "it",
        Lingua::En => "en",
    }
}

fn tabella() -> &'static HashMap<&'static str, &'static str> {
    static T: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    T.get_or_init(|| traduzioni::TESTI.iter().copied().collect())
}

/// Segna un testo che verrà tradotto più avanti con `t()` su una variabile
/// (es. valori salvati o usati come chiave): serve solo al test delle traduzioni.
pub const fn da_tradurre(s: &'static str) -> &'static str {
    s
}

/// Traduce un testo fisso. Un testo senza traduzione resta com'è.
pub fn t(it: &str) -> &str {
    match lingua() {
        Lingua::It => it,
        Lingua::En => tabella().get(it).copied().unwrap_or(it),
    }
}

/// Traduce un modello e sostituisce ogni `{}` con il valore corrispondente.
pub fn tf(it: &str, valori: &[&dyn Display]) -> String {
    let modello = t(it);
    let mut out = String::with_capacity(modello.len() + 16);
    let mut pezzi = modello.split("{}");
    if let Some(p) = pezzi.next() {
        out.push_str(p);
    }
    for (i, p) in pezzi.enumerate() {
        if let Some(v) = valori.get(i) {
            out.push_str(&v.to_string());
        }
        out.push_str(p);
    }
    out
}

/// Separatore decimale della lingua: «1,5» in italiano, «1.5» in inglese.
pub fn num(s: String) -> String {
    match lingua() {
        Lingua::It => s.replace('.', ","),
        Lingua::En => s,
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn modelli() {
        imposta(Lingua::It);
        assert_eq!(tf("{} su {}", &[&"hevc_vaapi", &"AMD"]), "hevc_vaapi su AMD");
        assert_eq!(num(format!("{:.1}", 1.5)), "1,5");
        imposta(Lingua::En);
        assert_eq!(tf("{} su {}", &[&"hevc_vaapi", &"AMD"]), "hevc_vaapi on AMD");
        assert_eq!(t("testo che non esiste"), "testo che non esiste");
        assert_eq!(num(format!("{:.1}", 1.5)), "1.5");
        imposta(Lingua::It);
    }

    /// Ogni testo passato a t() o tf() nel codice deve avere la traduzione inglese,
    /// con lo stesso numero di «{}», e la tabella non deve avere voci inutili o doppie.
    #[test]
    fn traduzioni_complete() {
        let mut usati = std::collections::BTreeSet::new();
        let radice = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut cartelle = vec![PathBuf::from(radice)];
        while let Some(d) = cartelle.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    cartelle.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("traduzioni.rs") && !p.ends_with("i18n.rs") {
                    estrai(&std::fs::read_to_string(&p).unwrap(), &mut usati);
                }
            }
        }
        let mut viste = std::collections::BTreeSet::new();
        for (it, en) in traduzioni::TESTI {
            assert!(viste.insert(*it), "voce doppia: {it:?}");
            assert_eq!(it.matches("{}").count(), en.matches("{}").count(), "segnaposto diversi: {it:?}");
        }
        let mancanti: Vec<_> = usati.iter().filter(|u| !viste.contains(u.as_str())).collect();
        assert!(mancanti.is_empty(), "senza traduzione: {mancanti:#?}");
        let inutili: Vec<_> = viste.iter().filter(|v| !usati.contains(**v)).collect();
        assert!(inutili.is_empty(), "traduzioni non usate: {inutili:#?}");
    }

    /// Trova i letterali in t("…"), tf("…", …) e da_tradurre("…"), anche spezzati su più righe con «\».
    fn estrai(sorgente: &str, usati: &mut std::collections::BTreeSet<String>) {
        for inizio in ["t(", "tf(", "da_tradurre("] {
            let mut resto = sorgente;
            while let Some(i) = resto.find(inizio) {
                // solo chiamate vere: il carattere prima non deve far parte di un identificatore
                let prima = resto[..i].chars().last();
                resto = &resto[i + inizio.len()..];
                if prima.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                // il testo può andare a capo dopo la parentesi
                let Some(dopo) = resto.trim_start().strip_prefix('"') else { continue };
                resto = dopo;
                let mut s = String::new();
                let mut it = resto.chars();
                while let Some(c) = it.next() {
                    match c {
                        '"' => break,
                        '\\' => match it.next() {
                            Some('n') => s.push('\n'),
                            Some('"') => s.push('"'),
                            Some('\\') => s.push('\\'),
                            Some('\n') => {
                                // continuazione di riga: salta gli spazi iniziali della riga dopo
                                let r = it.as_str().trim_start();
                                it = r.chars();
                            }
                            Some(x) => {
                                s.push('\\');
                                s.push(x);
                            }
                            None => {}
                        },
                        c => s.push(c),
                    }
                }
                usati.insert(s);
            }
        }
    }
}
