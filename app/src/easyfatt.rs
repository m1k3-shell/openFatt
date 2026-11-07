//! Importazione diretta di un archivio Easyfatt: si avvia il lettore ausiliario a 32 bit
//! (`openfatt-easyfatt.exe`, accanto a openfatt.exe) con il motore Firebird installato
//! insieme a Easyfatt, poi si importano i dati letti.

use std::path::{Path, PathBuf};

/// Cartelle in cui di solito si trova il motore Firebird di Easyfatt.
pub fn default_engine_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for base in [std::env::var_os("ProgramFiles(x86)"), std::env::var_os("ProgramFiles")].into_iter().flatten() {
        let base = PathBuf::from(base);
        if let Ok(entries) = std::fs::read_dir(&base) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name.contains("easyfatt") {
                    v.push(e.path().join("FirebirdEmbedded"));
                }
            }
        }
    }
    v.into_iter().filter(|p| p.join("fbembed.dll").exists() || p.join("fbclient.dll").exists()).collect()
}

pub fn helper_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidates = [dir.join("openfatt-easyfatt.exe"), dir.join("..").join("i686-pc-windows-msvc").join("release").join("openfatt-easyfatt.exe"), dir.join("..").join("..").join("target").join("i686-pc-windows-msvc").join("release").join("openfatt-easyfatt.exe")];
    candidates.into_iter().find(|p| p.exists())
}

/// Legge l'archivio con il lettore ausiliario e restituisce le tabelle.
pub fn read_archive(archive: &Path, engine: &Path) -> Result<openfatt_store::EasyfattDump, String> {
    let helper = helper_path().ok_or("Lettore openfatt-easyfatt.exe non trovato accanto a openfatt.exe")?;
    let out = std::env::temp_dir().join(format!("openfatt-easyfatt-{}.json", std::process::id()));
    let res = std::process::Command::new(&helper).arg(archive).arg(engine).arg(&out).output().map_err(|e| format!("Avvio del lettore: {}", e))?;
    if !res.status.success() {
        let msg = String::from_utf8_lossy(&res.stderr).trim().to_string();
        let _ = std::fs::remove_file(&out);
        return Err(if msg.is_empty() { "Lettura dell'archivio non riuscita".into() } else { msg });
    }
    let bytes = std::fs::read(&out).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&out);
    serde_json::from_slice(&bytes).map_err(|e| format!("Dati letti non validi: {}", e))
}

pub fn report_text(r: &openfatt_store::EasyfattImport) -> String {
    let mut t = format!(
        "Importati: {} clienti e fornitori ({} già presenti), {} articoli ({} già presenti), {} documenti, {} scadenze, {} incassi e pagamenti, {} movimenti di magazzino, {} modalità di pagamento, {} agenti, {} aliquote.",
        r.parties, r.parties_existing, r.products, r.products_existing, r.documents, r.dues, r.cash_entries, r.stock_moves, r.payments, r.agents, r.vat_codes
    );
    if r.documents_skipped > 0 {
        t.push_str(&format!("\n{} documenti non importati.", r.documents_skipped));
    }
    t.push_str("\n\nLe fatture importate restano bloccate: la fattura elettronica l'ha già trasmessa Easyfatt.");
    if !r.warnings.is_empty() {
        t.push_str("\n\nDa controllare:\n");
        for w in &r.warnings {
            t.push_str(&format!("•  {}\n", w));
        }
    }
    t
}
