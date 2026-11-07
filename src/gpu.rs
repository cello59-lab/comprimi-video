//! Elenco delle schede video presenti, letto da /sys (nessuna dipendenza esterna).

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Produttore {
    Amd,
    Intel,
    Nvidia,
    Altro,
}

impl Produttore {
    pub fn da_id(id: u16) -> Self {
        match id {
            0x1002 | 0x1022 => Self::Amd,
            0x8086 => Self::Intel,
            0x10de => Self::Nvidia,
            _ => Self::Altro,
        }
    }

    pub fn nome(self) -> &'static str {
        match self {
            Self::Amd => "AMD",
            Self::Intel => "Intel",
            Self::Nvidia => "NVIDIA",
            Self::Altro => "GPU",
        }
    }

    /// Sigla usata nel suffisso dei file prodotti (es. nome-720p-HEVC-AMD.mp4).
    pub fn sigla(self) -> &'static str {
        match self {
            Self::Amd => "AMD",
            Self::Intel => "INTEL",
            Self::Nvidia => "NVIDIA",
            Self::Altro => "GPU",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gpu {
    pub nome: String,
    pub produttore: Produttore,
    pub vendor_id: u16,
    pub device_id: u16,
    /// Indirizzo PCI, es. 0000:03:00.0
    pub pci: Option<String>,
    /// Nodo di rendering, es. /dev/dri/renderD128
    pub nodo: Option<String>,
    /// Driver del kernel: amdgpu, i915, xe, nvidia, nouveau…
    pub driver: Option<String>,
    /// L'utente può aprire il nodo (altrimenti manca il gruppo «render»)
    pub accessibile: bool,
    /// Scheda che ha avviato il sistema (boot_vga)
    pub principale: bool,
}

pub fn elenca() -> Vec<Gpu> {
    elenca_da(Path::new("/sys"), Path::new("/dev/dri"))
}

fn leggi_testo(p: &Path) -> String {
    fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn leggi_hex(p: &Path) -> Option<u32> {
    let s = leggi_testo(p);
    u32::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

fn nome_link(p: &Path) -> Option<String> {
    fs::read_link(p).ok()?.file_name()?.to_str().map(String::from)
}

/// Come `elenca`, ma con radici configurabili (usato nei test con un /sys finto).
pub fn elenca_da(sys: &Path, dev: &Path) -> Vec<Gpu> {
    let mut out: Vec<Gpu> = Vec::new();

    // 1. nodi di rendering DRM: sono quelli che VAAPI (e Vulkan) usano davvero
    let drm = sys.join("class/drm");
    let mut nodi: Vec<String> = fs::read_dir(&drm)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.starts_with("renderD"))
                .collect()
        })
        .unwrap_or_default();
    nodi.sort_by_key(|n| n[7..].parse::<u32>().unwrap_or(u32::MAX));

    for n in nodi {
        let d = drm.join(&n).join("device");
        let vendor = leggi_hex(&d.join("vendor")).unwrap_or(0) as u16;
        let device = leggi_hex(&d.join("device")).unwrap_or(0) as u16;
        let nodo = dev.join(&n);
        // aprire un nodo render non ha effetti: è il modo più affidabile per sapere se si hanno i permessi
        let accessibile = fs::OpenOptions::new().read(true).write(true).open(&nodo).is_ok();
        out.push(Gpu {
            nome: String::new(),
            produttore: Produttore::da_id(vendor),
            vendor_id: vendor,
            device_id: device,
            pci: nome_link(&d).filter(|s| s.contains(':')),
            nodo: Some(nodo.to_string_lossy().into_owned()),
            driver: nome_link(&d.join("driver")),
            accessibile,
            principale: leggi_testo(&d.join("boot_vga")) == "1",
        });
    }

    // 2. schede video PCI senza nodo render (es. NVIDIA con driver che non espone /dev/dri)
    if let Ok(rd) = fs::read_dir(sys.join("bus/pci/devices")) {
        let mut voci: Vec<_> = rd.flatten().collect();
        voci.sort_by_key(|e| e.file_name());
        for e in voci {
            let p = e.path();
            let slot = e.file_name().to_string_lossy().into_owned();
            let classe = leggi_hex(&p.join("class")).unwrap_or(0);
            if classe >> 16 != 0x03 {
                continue; // non è una scheda video
            }
            if out.iter().any(|g| g.pci.as_deref() == Some(slot.as_str())) {
                continue;
            }
            let vendor = leggi_hex(&p.join("vendor")).unwrap_or(0) as u16;
            let device = leggi_hex(&p.join("device")).unwrap_or(0) as u16;
            out.push(Gpu {
                nome: String::new(),
                produttore: Produttore::da_id(vendor),
                vendor_id: vendor,
                device_id: device,
                pci: Some(slot),
                nodo: None,
                driver: nome_link(&p.join("driver")),
                accessibile: false,
                principale: leggi_testo(&p.join("boot_vga")) == "1",
            });
        }
    }

    let ids = carica_pci_ids();
    for g in &mut out {
        g.nome = ids
            .as_deref()
            .and_then(|t| nome_pci(t, g.vendor_id, g.device_id))
            .map(|grezzo| nome_leggibile(g.produttore, &grezzo))
            .unwrap_or_else(|| {
                format!("{} ({:04x}:{:04x})", g.produttore.nome(), g.vendor_id, g.device_id)
            });
    }
    out
}

fn carica_pci_ids() -> Option<String> {
    [
        "/usr/share/hwdata/pci.ids",
        "/usr/share/misc/pci.ids",
        "/usr/share/pci.ids",
    ]
    .iter()
    .find_map(|p| fs::read_to_string(p).ok())
}

/// Cerca il nome del dispositivo nel database pci.ids.
pub fn nome_pci(testo: &str, vendor: u16, device: u16) -> Option<String> {
    let v = format!("{vendor:04x}");
    let d = format!("{device:04x}");
    let mut nel_vendor = false;
    for riga in testo.lines() {
        if riga.starts_with('#') || riga.is_empty() {
            continue;
        }
        if !riga.starts_with('\t') {
            if nel_vendor {
                return None; // finito il blocco del produttore senza trovare il dispositivo
            }
            nel_vendor = riga.starts_with(&v) && riga[4..].starts_with(' ');
        } else if nel_vendor && !riga.starts_with("\t\t") {
            let r = &riga[1..];
            if r.starts_with(&d) && r[4..].starts_with(' ') {
                return Some(r[4..].trim().to_string());
            }
        }
    }
    None
}

/// «Navi 23 [Radeon RX 6600/6600 XT/6600M]» → «AMD Radeon RX 6600/6600 XT/6600M (Navi 23)».
pub fn nome_leggibile(prod: Produttore, grezzo: &str) -> String {
    let (commerciale, interno) = match (grezzo.find('['), grezzo.rfind(']')) {
        (Some(a), Some(b)) if b > a => (grezzo[a + 1..b].trim(), grezzo[..a].trim()),
        _ => (grezzo.trim(), ""),
    };
    // pci.ids elenca tutta la famiglia («RX 7600/7600 XT/7600M XT…»): si tiene il primo modello
    let commerciale = commerciale.split('/').next().unwrap_or(commerciale).trim();
    let marca = prod.nome();
    let base = if prod == Produttore::Altro || commerciale.starts_with(marca) {
        commerciale.to_string()
    } else {
        format!("{marca} {commerciale}")
    };
    if interno.is_empty() {
        base
    } else {
        format!("{base} ({interno})")
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const IDS: &str = "# commento\n\
1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\
\t73ff  Navi 23 [Radeon RX 6600/6600 XT/6600M]\n\
\t\t1458 2329  Radeon RX 6600 Eagle\n\
\t1638  Cezanne [Radeon Vega Series / Radeon Vega Mobile Series]\n\
10de  NVIDIA Corporation\n\
\t2504  GA106 [GeForce RTX 3060 Lite Hash Rate]\n\
8086  Intel Corporation\n\
\t3e92  CoffeeLake-S GT2 [UHD Graphics 630]\n";

    #[test]
    fn trova_nomi() {
        assert_eq!(
            nome_pci(IDS, 0x1002, 0x73ff).as_deref(),
            Some("Navi 23 [Radeon RX 6600/6600 XT/6600M]")
        );
        assert_eq!(
            nome_pci(IDS, 0x8086, 0x3e92).as_deref(),
            Some("CoffeeLake-S GT2 [UHD Graphics 630]")
        );
        assert_eq!(nome_pci(IDS, 0x1002, 0x2504), None); // device di un altro produttore
        assert_eq!(nome_pci(IDS, 0x1234, 0x0001), None);
    }

    #[test]
    fn nomi_leggibili() {
        assert_eq!(
            nome_leggibile(Produttore::Amd, "Navi 23 [Radeon RX 6600/6600 XT/6600M]"),
            "AMD Radeon RX 6600 (Navi 23)"
        );
        assert_eq!(
            nome_leggibile(Produttore::Nvidia, "GA106 [GeForce RTX 3060 Lite Hash Rate]"),
            "NVIDIA GeForce RTX 3060 Lite Hash Rate (GA106)"
        );
        assert_eq!(nome_leggibile(Produttore::Intel, "Arc A380"), "Intel Arc A380");
        assert_eq!(
            nome_leggibile(Produttore::Amd, "Navi 33 [Radeon RX 7600/7600 XT/7600M XT/7600S/7700S / PRO W7600]"),
            "AMD Radeon RX 7600 (Navi 33)"
        );
    }

    #[test]
    fn sys_finto() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!("cv-test-sys-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let sys = base.join("sys");
        let dev = base.join("dev");
        fs::create_dir_all(&dev).unwrap();

        // scheda AMD con nodo render
        let pci_amd = sys.join("devices/pci0000:00/0000:03:00.0");
        fs::create_dir_all(&pci_amd).unwrap();
        fs::write(pci_amd.join("vendor"), "0x1002\n").unwrap();
        fs::write(pci_amd.join("device"), "0x73ff\n").unwrap();
        fs::write(pci_amd.join("class"), "0x030000\n").unwrap();
        fs::write(pci_amd.join("boot_vga"), "1\n").unwrap();
        fs::create_dir_all(sys.join("bus/pci/drivers/amdgpu")).unwrap();
        symlink(sys.join("bus/pci/drivers/amdgpu"), pci_amd.join("driver")).unwrap();
        fs::create_dir_all(sys.join("class/drm/renderD128")).unwrap();
        symlink(&pci_amd, sys.join("class/drm/renderD128/device")).unwrap();
        fs::write(dev.join("renderD128"), "").unwrap();

        // scheda NVIDIA senza nodo render
        let pci_nv = sys.join("bus/pci/devices/0000:01:00.0");
        fs::create_dir_all(&pci_nv).unwrap();
        fs::write(pci_nv.join("vendor"), "0x10de\n").unwrap();
        fs::write(pci_nv.join("device"), "0x2504\n").unwrap();
        fs::write(pci_nv.join("class"), "0x030200\n").unwrap();
        symlink(&pci_amd, sys.join("bus/pci/devices/0000:03:00.0")).unwrap();
        // una scheda di rete, da ignorare
        let pci_net = sys.join("bus/pci/devices/0000:05:00.0");
        fs::create_dir_all(&pci_net).unwrap();
        fs::write(pci_net.join("class"), "0x020000\n").unwrap();

        let g = elenca_da(&sys, &dev);
        assert_eq!(g.len(), 2, "{g:#?}");
        assert_eq!(g[0].produttore, Produttore::Amd);
        assert_eq!(g[0].pci.as_deref(), Some("0000:03:00.0"));
        assert_eq!(g[0].driver.as_deref(), Some("amdgpu"));
        assert!(g[0].accessibile && g[0].principale);
        assert_eq!(g[1].produttore, Produttore::Nvidia);
        assert!(g[1].nodo.is_none() && !g[1].accessibile);
        let _ = fs::remove_dir_all(&base);
    }
}
