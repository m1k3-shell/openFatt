//! Nuove versioni: all'avvio si chiede a GitHub qual è l'ultima Release pubblicata. Su richiesta
//! dell'utente il programma scarica l'installatore, ne verifica la firma e si aggiorna.
//!
//! Alla richiesta non viene aggiunto alcun dato dell'archivio. Se la rete manca non succede niente.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use openfatt_update::Manifest;

use crate::ui::theme;

/// Si usa il numero del repository e non il suo nome, che può cambiare o passare ad altri.
#[cfg(not(feature = "prova-aggiornamento"))]
const LATEST_RELEASE: &str = "https://api.github.com/repositories/1449638515/releases/latest";
/// Chiave pubblica con cui si controllano gli aggiornamenti.
#[cfg(not(feature = "prova-aggiornamento"))]
const PUBLIC_KEY: &str = "eb2b2ccc31dcc9fe222b79c8ff926b113cf7bd1cbec22abfcbff4b9fcedb9d37";
#[cfg(feature = "prova-aggiornamento")]
const PUBLIC_KEY: &str = env!("OPENFATT_PROVA_CHIAVE");
/// Se esiste questo file nelle preferenze, il controllo è spento.
const OFF_MARKER: &str = "nessun-controllo-versione";

fn latest_url() -> String {
    #[cfg(feature = "prova-aggiornamento")]
    return std::env::var("OPENFATT_PROVA_API").unwrap_or_default();
    #[cfg(not(feature = "prova-aggiornamento"))]
    LATEST_RELEASE.to_string()
}

/// Indirizzi dei file di una versione.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    manifest_url: String,
    installer_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub version: String,
    pub url: String,
    /// Assente nelle versioni pubblicate senza manifesto firmato: si apre la pagina.
    pub package: Option<Package>,
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Downloading {
        rx: Receiver<Result<(PathBuf, Manifest), String>>,
        progress: Arc<AtomicU64>,
    },
    Ready(PathBuf, Manifest),
    Failed(String),
}

/// Scelta fatta sull'avviso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Download,
    Install,
    OpenPage,
    Hide,
}

#[derive(Default)]
pub struct Checker {
    rx: Option<Receiver<Option<Update>>>,
    found: Option<Update>,
    hidden: bool,
    phase: Phase,
}

fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    let v = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    parts.next().is_none().then_some(v)
}

fn is_newer(current: &str, latest: &str) -> bool {
    matches!((parse_version(current), parse_version(latest)), (Some(c), Some(l)) if l > c)
}

/// L'indirizzo arriva dalla rete e viene passato al sistema: si accetta solo una pagina di
/// Release di GitHub, senza caratteri che la riga di comando potrebbe interpretare.
fn is_release_page(url: &str) -> bool {
    let Some(path) = url.strip_prefix("https://github.com/") else { return false };
    let parts: Vec<&str> = path.split('/').collect();
    parts.len() == 5 && parts[1] == "openFatt" && parts[2] == "releases" && parts[3] == "tag" && url.chars().all(|c| c.is_ascii_alphanumeric() || "/:.-_".contains(c))
}

/// File allegato a una Release del progetto. La sua autenticità la garantisce la firma.
fn is_asset_url(url: &str) -> bool {
    #[cfg(feature = "prova-aggiornamento")]
    if url.starts_with("http://127.0.0.1:") {
        return true;
    }
    let Some(path) = url.strip_prefix("https://github.com/") else { return false };
    let parts: Vec<&str> = path.split('/').collect();
    parts.len() == 6 && parts[1] == "openFatt" && parts[2] == "releases" && parts[3] == "download"
}

fn asset_url(json: &serde_json::Value, name: &str) -> Option<String> {
    json.get("assets")?.as_array()?.iter().find(|a| a.get("name").and_then(|n| n.as_str()) == Some(name))?.get("browser_download_url")?.as_str().filter(|u| is_asset_url(u)).map(str::to_string)
}

fn from_response(body: &str, current: &str) -> Option<Update> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let tag = json.get("tag_name")?.as_str()?;
    let url = json.get("html_url")?.as_str()?;
    if !is_newer(current, tag) || !is_release_page(url) {
        return None;
    }
    let version = tag.trim_start_matches('v').to_string();
    let package = (|| Some(Package { manifest_url: asset_url(&json, &format!("openFatt-Update-{}.json", version))?, installer_url: asset_url(&json, &format!("openFatt-Setup-{}.exe", version))? }))();
    Some(Update { version, url: url.to_string(), package })
}

/// Versione di questo programma (OPENFATT_VERSIONE la sostituisce nelle prove).
pub fn current() -> String {
    std::env::var("OPENFATT_VERSIONE").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}

fn fetch() -> Option<Update> {
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(10)).build();
    let body = agent.get(&latest_url()).set("User-Agent", concat!("openFatt/", env!("CARGO_PKG_VERSION"))).set("Accept", "application/vnd.github+json").call().ok()?.into_string().ok()?;
    from_response(&body, &current())
}

fn off_marker() -> std::path::PathBuf {
    crate::config::config_dir().join(OFF_MARKER)
}

pub fn enabled() -> bool {
    !off_marker().exists()
}

pub fn set_enabled(on: bool) {
    if on {
        let _ = std::fs::remove_file(off_marker());
    } else {
        let _ = std::fs::create_dir_all(crate::config::config_dir());
        let _ = std::fs::write(off_marker(), b"");
    }
}

impl Checker {
    /// Il controllo gira in sottofondo: la finestra si apre senza aspettare la rete.
    pub fn start(ctx: &egui::Context) -> Self {
        clean_downloads();
        if !enabled() {
            return Self::default();
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(fetch());
            ctx.request_repaint();
        });
        Checker { rx: Some(rx), ..Self::default() }
    }

    pub fn poll(&mut self) {
        if let Some(found) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.found = found;
            self.rx = None;
        }
        if let Phase::Downloading { rx, .. } = &self.phase {
            if let Ok(result) = rx.try_recv() {
                self.phase = match result {
                    Ok((path, manifest)) => Phase::Ready(path, manifest),
                    Err(e) => Phase::Failed(e),
                };
            }
        }
    }

    pub fn hide(&mut self) {
        self.hidden = true;
    }

    /// Avvia lo scaricamento in sottofondo.
    pub fn download(&mut self, ctx: &egui::Context) {
        let Some(update) = self.found.clone() else { return };
        let Some(package) = update.package.clone() else { return };
        let (tx, rx) = channel();
        let progress = Arc::new(AtomicU64::new(0));
        let (ctx, p) = (ctx.clone(), progress.clone());
        std::thread::spawn(move || {
            let _ = tx.send(fetch_package(&update.version, &package, &p));
            ctx.request_repaint();
        });
        self.phase = Phase::Downloading { rx, progress };
    }

    /// File pronto, ricontrollato subito prima dell'uso: (percorso, versione).
    pub fn checked_installer(&mut self) -> Result<(PathBuf, String), String> {
        let Phase::Ready(path, manifest) = &self.phase else { return Err("Aggiornamento non pronto".into()) };
        let checked = openfatt_update::verify(manifest, PUBLIC_KEY).and_then(|_| openfatt_update::check_file(path, manifest));
        match checked {
            Ok(()) => Ok((path.clone(), manifest.version.clone())),
            Err(e) => {
                self.phase = Phase::Failed(e.clone());
                Err(e)
            }
        }
    }

    pub fn open_page(&self) {
        if let Some(u) = &self.found {
            crate::services::open_path(Path::new(&u.url));
        }
    }

    /// Avviso in alto. `can_update`: l'utente può installare aggiornamenti.
    pub fn banner(&self, ctx: &egui::Context, can_update: bool) -> Option<Action> {
        let update = self.found.as_ref().filter(|_| !self.hidden)?;
        if matches!(self.phase, Phase::Downloading { .. }) {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        let self_update = can_update && update.package.is_some() && installed_with_installer();
        let mut action = None;
        egui::TopBottomPanel::top("update_banner").frame(egui::Frame::new().fill(theme::CURRENT).inner_margin(egui::Margin::symmetric(12, 5))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                match &self.phase {
                    Phase::Idle => {
                        ui.label(format!("È disponibile openFatt {} (questo computer ha la {}).", update.version, current()));
                    }
                    Phase::Downloading { progress, .. } => {
                        ui.spinner();
                        ui.label(format!("Scaricamento di openFatt {}: {:.1} MB. Puoi continuare a lavorare.", update.version, progress.load(Ordering::Relaxed) as f64 / 1_048_576.0));
                    }
                    Phase::Ready(..) => {
                        ui.label(format!("openFatt {} è pronto e verificato. Al riavvio l'archivio viene salvato e il programma si aggiorna.", update.version));
                    }
                    Phase::Failed(e) => {
                        ui.label(egui::RichText::new(format!("{}  Aggiornamento non riuscito: {}", theme::icon::WARNING, e)).color(theme::WARNING));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match &self.phase {
                        Phase::Idle if self_update => {
                            if theme::primary(ui, theme::icon::DOWNLOAD_SIMPLE, "Aggiorna ora").clicked() {
                                action = Some(Action::Download);
                            }
                        }
                        Phase::Idle => {
                            if theme::ghost(ui, true, theme::icon::ARROW_RIGHT, "Scarica").clicked() {
                                action = Some(Action::OpenPage);
                            }
                        }
                        Phase::Ready(..) => {
                            if theme::primary(ui, theme::icon::ARROWS_CLOCKWISE, "Riavvia e aggiorna").clicked() {
                                action = Some(Action::Install);
                            }
                        }
                        Phase::Failed(_) => {
                            if theme::primary(ui, theme::icon::ARROWS_CLOCKWISE, "Riprova").clicked() {
                                action = Some(Action::Download);
                            }
                        }
                        Phase::Downloading { .. } => {}
                    }
                    if !matches!(self.phase, Phase::Downloading { .. }) && ui.small_button("Più tardi").clicked() {
                        action = Some(Action::Hide);
                    }
                    if matches!(self.phase, Phase::Failed(_)) && ui.small_button("Apri la pagina").clicked() {
                        action = Some(Action::OpenPage);
                    }
                });
            });
        });
        action
    }
}

fn update_dir() -> PathBuf {
    crate::config::config_dir().join("aggiornamenti")
}

/// Scarica manifesto e installatore: restituisce il file solo se la firma è valida.
fn fetch_package(version: &str, package: &Package, progress: &AtomicU64) -> Result<(PathBuf, Manifest), String> {
    use std::io::Read;
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(60)).build();
    let get = |url: &str| agent.get(url).set("User-Agent", concat!("openFatt/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| format!("Connessione non riuscita: {}", e));
    let mut text = String::new();
    get(&package.manifest_url)?.into_reader().take(openfatt_update::MAX_MANIFEST).read_to_string(&mut text).map_err(|e| e.to_string())?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|_| "Descrizione dell'aggiornamento non valida".to_string())?;
    check_manifest(&manifest, version, &current())?;
    let dir = update_dir();
    let _ = std::fs::remove_dir_all(&dir);
    let path = openfatt_update::save_verified(get(&package.installer_url)?.into_reader(), &manifest, &dir, progress)?;
    Ok((path, manifest))
}

/// Firma valida, versione attesa e mai una versione precedente a quella in uso.
fn check_manifest(m: &Manifest, expected: &str, current: &str) -> Result<(), String> {
    openfatt_update::verify(m, PUBLIC_KEY)?;
    if m.version != expected || m.file != format!("openFatt-Setup-{}.exe", expected) {
        return Err("L'aggiornamento firmato non è quello atteso".into());
    }
    if !is_newer(current, &m.version) {
        return Err("L'aggiornamento non è più recente della versione in uso".into());
    }
    Ok(())
}

/// Solo un programma messo dall'installatore si può aggiornare da sé.
fn installed_with_installer() -> bool {
    std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join("Disinstalla.exe").exists())).unwrap_or(false)
}

/// Avvia l'installatore, che attende la chiusura del programma e poi lo riapre.
pub fn launch_installer(path: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("Cartella del programma non trovata")?;
    let mut command = std::process::Command::new(path);
    command.arg("/S").arg("/RIAVVIA");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // l'installatore vuole /D= per ultimo e senza virgolette, anche con spazi nel percorso
        command.raw_arg(format!("/D={}", dir.display()));
    }
    #[cfg(not(windows))]
    command.arg(format!("/D={}", dir.display()));
    command.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// All'avvio si eliminano i file di aggiornamenti già conclusi o interrotti.
pub fn clean_downloads() {
    let _ = std::fs::remove_dir_all(update_dir());
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "https://github.com/m1k3-shell/openFatt/releases/tag/v0.2.0";

    #[test]
    fn confronta_i_numeri_e_non_il_testo() {
        assert!(is_newer("0.1.0", "v0.1.1"));
        assert!(is_newer("0.9.0", "0.10.0"));
        assert!(is_newer("0.9.9", "v1.0.0"));
        assert!(!is_newer("0.1.0", "v0.1.0"));
        assert!(!is_newer("0.2.0", "v0.1.9"));
    }

    #[test]
    fn un_numero_non_valido_non_genera_avvisi() {
        assert!(!is_newer("0.1.0", "ultima"));
        assert!(!is_newer("0.1.0", "v0.2"));
        assert!(!is_newer("0.1.0", "v0.2.0-beta"));
        assert!(!is_newer("0.1.0", "v0.2.0.1"));
    }

    #[test]
    fn accetta_solo_le_pagine_di_release_del_progetto() {
        assert!(is_release_page(PAGE));
        for bad in [
            "http://github.com/m1k3-shell/openFatt/releases/tag/v0.2.0",
            "https://github.com.example.org/a/openFatt/releases/tag/v0.2.0",
            "https://github.com/m1k3-shell/altro/releases/tag/v0.2.0",
            "https://github.com/m1k3-shell/openFatt/releases/download/v0.2.0/virus.exe",
            "https://github.com/m1k3-shell/openFatt/releases/tag/v0.2.0&calc.exe",
            "https://github.com/m1k3-shell/openFatt/releases/tag/v0.2.0 calc",
        ] {
            assert!(!is_release_page(bad), "{}", bad);
        }
    }

    const DL: &str = "https://github.com/m1k3-shell/openFatt/releases/download/v0.2.0/";

    #[test]
    fn accetta_solo_gli_allegati_del_progetto() {
        assert!(is_asset_url(&format!("{}openFatt-Setup-0.2.0.exe", DL)));
        for bad in ["http://github.com/a/openFatt/releases/download/v1/x.exe", "https://github.com.evil.org/a/openFatt/releases/download/v1/x.exe", "https://github.com/a/altro/releases/download/v1/x.exe", "https://example.org/x.exe", "https://github.com/a/openFatt/releases/tag/v1/x.exe"] {
            assert!(!is_asset_url(bad), "{}", bad);
        }
    }

    #[test]
    fn trova_i_file_della_versione() {
        let asset = |n: &str| format!(r#"{{"name":"{n}","browser_download_url":"{DL}{n}"}}"#);
        let body = format!(r#"{{"tag_name":"v0.2.0","html_url":"{}","assets":[{},{}]}}"#, PAGE, asset("openFatt-Setup-0.2.0.exe"), asset("openFatt-Update-0.2.0.json"));
        let p = from_response(&body, "0.1.0").unwrap().package.unwrap();
        assert!(p.manifest_url.ends_with("openFatt-Update-0.2.0.json") && p.installer_url.ends_with("openFatt-Setup-0.2.0.exe"));
        // senza manifesto si torna alla pagina
        let body = format!(r#"{{"tag_name":"v0.2.0","html_url":"{}","assets":[{}]}}"#, PAGE, asset("openFatt-Setup-0.2.0.exe"));
        assert_eq!(from_response(&body, "0.1.0").unwrap().package, None);
    }

    #[test]
    fn legge_la_risposta_di_github() {
        let body = format!(r#"{{"tag_name":"v0.2.0","html_url":"{}","draft":false}}"#, PAGE);
        assert_eq!(from_response(&body, "0.1.0"), Some(Update { version: "0.2.0".into(), url: PAGE.into(), package: None }));
        assert_eq!(from_response(&body, "0.2.0"), None);
        assert_eq!(from_response(r#"{"message":"Not Found"}"#, "0.1.0"), None);
        assert_eq!(from_response("<html>errore</html>", "0.1.0"), None);
    }
}
