//! Tempi e tagli (porting fedele della versione Python).

pub type Taglio = (f64, f64);

/// 83.5 → «01:23.5», 3700 → «1:01:40.0»
pub fn fmt_t(s: f64) -> String {
    let s = s.max(0.0);
    let h = (s / 3600.0) as u64;
    let m = ((s % 3600.0) / 60.0) as u64;
    let sec = s % 60.0;
    if h > 0 {
        format!("{h}:{m:02}:{sec:04.1}")
    } else {
        format!("{m:02}:{sec:04.1}")
    }
}

/// «83», «1:23», «1:23,5», «0:01:23.5» → secondi.
pub fn parse_tempo(t: &str) -> Result<f64, String> {
    let t = t.trim().replace(',', ".");
    let parti: Vec<&str> = t.split(':').collect();
    if t.is_empty() || parti.len() > 3 {
        return Err(format!("tempo non valido: «{t}»"));
    }
    let mut sec = 0.0;
    for p in parti {
        let v: f64 = p.parse().map_err(|_| format!("tempo non valido: «{t}»"))?;
        if !v.is_finite() || v < 0.0 {
            return Err(format!("tempo non valido: «{t}»"));
        }
        sec = sec * 60.0 + v;
    }
    Ok(sec)
}

/// «1:00-1:30» → (60, 90)
pub fn parse_taglio(t: &str) -> Result<Taglio, String> {
    let (a, b) = t
        .split_once('-')
        .ok_or_else(|| format!("taglio non valido: «{t}» (formato DA-A, es. 1:00-1:30)"))?;
    let (a, b) = (parse_tempo(a)?, parse_tempo(b)?);
    if b <= a + 0.05 {
        return Err(format!("taglio non valido: «{t}» (la fine deve venire dopo l'inizio)"));
    }
    Ok((a, b))
}

/// Ordina e fonde i tagli sovrapposti.
pub fn unisci_tagli(tagli: &[Taglio]) -> Vec<Taglio> {
    let mut ordinati = tagli.to_vec();
    ordinati.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<Taglio> = Vec::new();
    for (a, b) in ordinati {
        match out.last_mut() {
            Some(u) if a <= u.1 + 0.01 => u.1 = u.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Complementare dei tagli: [(inizio, fine)], fine = None vuol dire «fino alla fine del video».
pub fn parti_tenute(tagli: &[Taglio], durata: f64) -> Vec<(f64, Option<f64>)> {
    let mut tenute = Vec::new();
    let mut pos = 0.0f64;
    for (a, b) in unisci_tagli(tagli) {
        if a > pos + 0.01 {
            tenute.push((pos, Some(a)));
        }
        pos = pos.max(b);
    }
    if durata <= 0.0 || pos < durata - 0.05 {
        tenute.push((pos, None));
    }
    tenute
}

/// Durata del risultato dopo i tagli (0 se la durata non è nota).
pub fn durata_dopo_tagli(tagli: &[Taglio], durata: f64) -> f64 {
    if durata <= 0.0 {
        return 0.0;
    }
    parti_tenute(tagli, durata)
        .iter()
        .map(|(a, b)| b.unwrap_or(durata).min(durata) - a)
        .filter(|d| *d > 0.0)
        .sum()
}

/// Quanto tolgono i tagli (somma dei tagli fusi se la durata non è nota).
pub fn durata_tagliata(tagli: &[Taglio], durata: f64) -> f64 {
    if durata > 0.0 {
        durata - durata_dopo_tagli(tagli, durata)
    } else {
        unisci_tagli(tagli).iter().map(|(a, b)| b - a).sum()
    }
}

/// filter_complex che tiene solo le parti indicate e le unisce; esce su [vo] (e [ac]).
pub fn filtro_con_tagli(tenute: &[(f64, Option<f64>)], con_audio: bool, filtro_video: &str) -> String {
    let k = tenute.len();
    let mut p: Vec<String> = Vec::new();
    if k > 1 {
        let uscite: String = (0..k).map(|i| format!("[vs{i}]")).collect();
        p.push(format!("[0:v:0]split={k}{uscite}"));
        if con_audio {
            let uscite: String = (0..k).map(|i| format!("[as{i}]")).collect();
            p.push(format!("[0:a:0]asplit={k}{uscite}"));
        }
    }
    for (i, (a, b)) in tenute.iter().enumerate() {
        let vin = if k > 1 { format!("[vs{i}]") } else { "[0:v:0]".into() };
        let ain = if k > 1 { format!("[as{i}]") } else { "[0:a:0]".into() };
        let mut lim = format!("start={a:.3}");
        if let Some(b) = b {
            lim += &format!(":end={b:.3}");
        }
        p.push(format!("{vin}trim={lim},setpts=PTS-STARTPTS[v{i}]"));
        if con_audio {
            p.push(format!("{ain}atrim={lim},asetpts=PTS-STARTPTS[a{i}]"));
        }
    }
    let ingressi: String = (0..k)
        .map(|i| if con_audio { format!("[v{i}][a{i}]") } else { format!("[v{i}]") })
        .collect();
    let a = if con_audio { 1 } else { 0 };
    let ac = if con_audio { "[ac]" } else { "" };
    p.push(format!("{ingressi}concat=n={k}:v=1:a={a}[vc]{ac}"));
    p.push(format!("[vc]{filtro_video}[vo]"));
    p.join(";")
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn formato_tempi() {
        assert_eq!(fmt_t(83.5), "01:23.5");
        assert_eq!(fmt_t(5.0), "00:05.0");
        assert_eq!(fmt_t(3700.0), "1:01:40.0");
        assert_eq!(fmt_t(-3.0), "00:00.0");
    }

    #[test]
    fn lettura_tempi() {
        assert_eq!(parse_tempo("83").unwrap(), 83.0);
        assert_eq!(parse_tempo("1:23").unwrap(), 83.0);
        assert_eq!(parse_tempo("1:23,5").unwrap(), 83.5);
        assert_eq!(parse_tempo("0:01:23.5").unwrap(), 83.5);
        assert!(parse_tempo("").is_err());
        assert!(parse_tempo("1:2:3:4").is_err());
        assert!(parse_tempo("abc").is_err());
        assert!(parse_tempo("-5").is_err());
        assert_eq!(parse_taglio("1:00-1:30").unwrap(), (60.0, 90.0));
        assert!(parse_taglio("1:30-1:00").is_err());
        assert!(parse_taglio("1:30").is_err());
    }

    #[test]
    fn tagli() {
        assert_eq!(unisci_tagli(&[(10.0, 20.0), (5.0, 12.0), (30.0, 40.0)]), vec![(5.0, 20.0), (30.0, 40.0)]);
        assert_eq!(
            parti_tenute(&[(10.0, 20.0), (30.0, 40.0)], 60.0),
            vec![(0.0, Some(10.0)), (20.0, Some(30.0)), (40.0, None)]
        );
        assert_eq!(parti_tenute(&[(0.0, 10.0)], 60.0), vec![(10.0, None)]);
        assert_eq!(parti_tenute(&[(50.0, 60.0)], 60.0), vec![(0.0, Some(50.0))]);
        assert!(parti_tenute(&[(0.0, 60.0)], 60.0).is_empty());
        assert!((durata_dopo_tagli(&[(10.0, 20.0), (15.0, 25.0)], 60.0) - 45.0).abs() < 1e-9);
    }

    #[test]
    fn filtro() {
        let f = filtro_con_tagli(&[(0.0, Some(10.0)), (20.0, None)], true, "format=yuv420p");
        assert_eq!(
            f,
            "[0:v:0]split=2[vs0][vs1];[0:a:0]asplit=2[as0][as1];\
             [vs0]trim=start=0.000:end=10.000,setpts=PTS-STARTPTS[v0];\
             [as0]atrim=start=0.000:end=10.000,asetpts=PTS-STARTPTS[a0];\
             [vs1]trim=start=20.000,setpts=PTS-STARTPTS[v1];\
             [as1]atrim=start=20.000,asetpts=PTS-STARTPTS[a1];\
             [v0][a0][v1][a1]concat=n=2:v=1:a=1[vc][ac];[vc]format=yuv420p[vo]"
        );
        let f = filtro_con_tagli(&[(5.0, None)], false, "x");
        assert_eq!(f, "[0:v:0]trim=start=5.000,setpts=PTS-STARTPTS[v0];[v0]concat=n=1:v=1:a=0[vc];[vc]x[vo]");
    }
}
