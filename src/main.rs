//! Comprimi Video — finestra GTK4/libadwaita, oppure uso da terminale con --cli.

#![allow(dead_code)] // alcune funzioni di supporto servono solo a una delle due interfacce

mod cache;
mod cli;
mod encoder;
mod ffmpeg;
mod gpu;
mod gui;
mod lavoro;
mod menu;
mod rileva;
mod stampa;
mod tempo;

use std::process::ExitCode;

/// Opzioni che portano all'uso da terminale.
const DA_TERMINALE: &[&str] = &["--cli", "--rileva", "--prova", "--aiuto", "--help", "-h"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ha = |s: &str| args.iter().any(|a| a == s);

    if ha("--versione") || ha("--version") {
        println!("Comprimi Video {}", gui::VERSIONE);
        return ExitCode::SUCCESS;
    }
    if ha("--installa-menu") {
        return menu::installa();
    }
    if ha("--rimuovi-menu") {
        return menu::rimuovi();
    }
    if args.iter().any(|a| DA_TERMINALE.contains(&a.as_str())) {
        return cli::principale(args);
    }
    if let Some(o) = args.iter().find(|a| a.starts_with('-')) {
        eprintln!("L'opzione {o} vale solo da terminale: aggiungi --cli (vedi --aiuto)");
        return ExitCode::from(1);
    }
    gui::avvia(args)
}
