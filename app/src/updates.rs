//! Avviso di nuova versione: all'avvio si chiede a GitHub qual è l'ultima Release pubblicata.
//!
//! Non si scarica né si installa nulla: l'utente vede un avviso e apre la pagina della Release.
//! Alla richiesta non viene aggiunto alcun dato dell'archivio. Se la rete manca non succede niente.

use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

/// Si usa il numero del repository e non il suo nome, che può cambiare o passare ad altri.
const LATEST_RELEASE: &str = "https://api.github.com/repositories/1449638515/releases/latest";
/// Se esiste questo file nelle preferenze, il controllo è spento.
const OFF_MARKER: &str = "nessun-controllo-versione";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub version: String,
    pub url: String,
}

#[derive(Default)]
pub struct Checker {
    rx: Option<Receiver<Option<Update>>>,
    found: Option<Update>,
    hidden: bool,
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

fn from_response(body: &str, current: &str) -> Option<Update> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let tag = json.get("tag_name")?.as_str()?;
    let url = json.get("html_url")?.as_str()?;
    (is_newer(current, tag) && is_release_page(url)).then(|| Update { version: tag.trim_start_matches('v').to_string(), url: url.to_string() })
}

/// Versione di questo programma (OPENFATT_VERSIONE la sostituisce nelle prove).
pub fn current() -> String {
    std::env::var("OPENFATT_VERSIONE").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}

fn fetch() -> Option<Update> {
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(10)).build();
    let body = agent.get(LATEST_RELEASE).set("User-Agent", concat!("openFatt/", env!("CARGO_PKG_VERSION"))).set("Accept", "application/vnd.github+json").call().ok()?.into_string().ok()?;
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
    pub fn start(ctx: &eframe::egui::Context) -> Self {
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
    }

    pub fn notice(&self) -> Option<&Update> {
        self.found.as_ref().filter(|_| !self.hidden)
    }

    pub fn hide(&mut self) {
        self.hidden = true;
    }
}

pub fn open_page(update: &Update) {
    crate::services::open_path(Path::new(&update.url));
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

    #[test]
    fn legge_la_risposta_di_github() {
        let body = format!(r#"{{"tag_name":"v0.2.0","html_url":"{}","draft":false}}"#, PAGE);
        assert_eq!(from_response(&body, "0.1.0"), Some(Update { version: "0.2.0".into(), url: PAGE.into() }));
        assert_eq!(from_response(&body, "0.2.0"), None);
        assert_eq!(from_response(r#"{"message":"Not Found"}"#, "0.1.0"), None);
        assert_eq!(from_response("<html>errore</html>", "0.1.0"), None);
    }
}
