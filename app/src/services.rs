//! Operazioni applicative che combinano archivio, calcolo, FatturaPA, PDF e PEC.

use std::path::{Path, PathBuf};

use openfatt_core::calc;
use openfatt_core::model::{DocStatus, Document};
use openfatt_fatturapa::{naming, BuildOptions, DdtRef, Generated, Severity};
use openfatt_pec::{InboundMessage, PecConfig};
use openfatt_store::{EInvoice, Store};

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FeOptions {
    /// Allega il PDF di cortesia alla fattura elettronica.
    pub attach_pdf: bool,
    /// CodiceTipo per CodiceArticolo (vuoto = INTERNO).
    pub article_code_type: String,
}

pub fn fe_options(store: &Store) -> FeOptions {
    store.get_setting("fe_options").unwrap_or_default()
}

/// Impostazioni della firma digitale.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SignConfig {
    pub source: openfatt_sign::SignSource,
    /// Firma automaticamente le fatture verso la PA appena generate (solo certificati dell'archivio/smart card).
    pub auto_pa: bool,
    /// Firma automaticamente anche le fatture verso privati.
    pub auto_all: bool,
}

pub fn sign_config(store: &Store) -> SignConfig {
    store.get_setting("signature").unwrap_or_default()
}

/// Firma la fattura elettronica e ne salva la versione .p7m.
pub fn sign_einvoice(store: &Store, einvoice_id: i64, pfx_password: &str) -> Result<EInvoice, String> {
    let cfg = sign_config(store);
    let e = store.einvoice(einvoice_id).map_err(|e| e.to_string())?;
    let p7m = openfatt_sign::sign(&e.xml, &cfg.source, pfx_password).map_err(|e| e.0)?;
    // controllo immediato: la busta prodotta deve verificarsi e contenere esattamente l'XML
    let v = openfatt_sign::verify(&p7m).map_err(|e| format!("La firma prodotta non supera la verifica: {}", e))?;
    if v.content != e.xml || !v.cades {
        return Err("La firma prodotta non è conforme (CAdES-BES)".into());
    }
    store.attach_signed_file(einvoice_id, &p7m).map_err(|e| e.to_string())
}

/// Carica una fattura firmata con un programma esterno (Dike, ArubaSign, ...).
pub fn attach_external_signature(store: &Store, einvoice_id: i64, p7m: &[u8]) -> Result<String, String> {
    let note = match openfatt_sign::verify(p7m) {
        Ok(v) => format!("Firma verificata: {} (certificato valido fino al {})", v.signer.subject, v.signer.valid_to),
        Err(e) => return Err(format!("Firma non valida: {}", e)),
    };
    store.attach_signed_file(einvoice_id, p7m).map_err(|e| e.to_string())?;
    Ok(note)
}

pub fn pec_config(store: &Store) -> PecConfig {
    let c: PecConfig = store.get_setting("pec").unwrap_or_default();
    c
}

/// Genera (senza salvarla) la fattura elettronica di un documento emesso.
pub fn build_einvoice(store: &Store, doc: &Document, progressivo: &str) -> Result<Generated, String> {
    if doc.status != DocStatus::Issued {
        return Err("Emettere il documento prima di generare la fattura elettronica".into());
    }
    let company = store.company().map_err(|e| e.to_string())?;
    let totals = calc::compute(doc).map_err(|e| e.0)?;
    let ddt = store.ddt_sources(doc).map_err(|e| e.to_string())?;
    let opts = fe_options(store);
    let attachment = if opts.attach_pdf {
        let pdf = crate::printing::document_pdf(store, doc)?;
        Some((format!("{}_{}.pdf", doc.kind.code(), doc.number_label().replace('/', "-")), pdf))
    } else {
        None
    };
    let build = BuildOptions {
        progressivo: progressivo.to_string(),
        ddt_refs: ddt.into_iter().map(|d| DdtRef { number: d.number, date: Some(d.date), line_indexes: d.line_indexes }).collect(),
        attachment,
        article_code_type: opts.article_code_type,
    };
    openfatt_fatturapa::generate(&company, doc, &totals, &build).map_err(|e| e.0)
}

/// Identificativo del trasmittente usato nel nome file.
fn transmitter_id(store: &Store) -> (String, String) {
    let c = store.company().unwrap_or_default();
    if !c.transmitter_code.trim().is_empty() {
        let country = if c.transmitter_country.is_empty() { "IT".to_string() } else { c.transmitter_country.clone() };
        return (country, c.transmitter_code.trim().to_uppercase());
    }
    let cf = c.tax_code.trim().to_uppercase();
    if cf.len() == 16 {
        ("IT".into(), cf)
    } else {
        ("IT".into(), c.vat_number.trim().to_uppercase())
    }
}

/// Controlla, numera e registra la fattura elettronica di un documento.
pub fn create_einvoice(store: &Store, doc_id: i64) -> Result<(EInvoice, Vec<openfatt_fatturapa::Issue>), String> {
    let doc = store.document(doc_id).map_err(|e| e.to_string())?;
    let next = store.peek_counter("fe_progressivo").map_err(|e| e.to_string())? + 1;
    let prog = naming::progressive(next);
    let g = build_einvoice(store, &doc, &prog)?;
    if !g.is_sendable() {
        let list: Vec<String> = g.issues.iter().filter(|i| i.severity == Severity::Error).map(|i| i.to_string()).collect();
        return Err(format!("La fattura non supera i controlli SdI:\n{}", list.join("\n")));
    }
    store.next_counter("fe_progressivo").map_err(|e| e.to_string())?;
    let (country, code) = transmitter_id(store);
    let name = naming::invoice_file_name(&country, &code, &prog);
    let mut e = store.create_outgoing_einvoice(&doc, &name, &g.xml, &g.fattura).map_err(|e| e.to_string())?;
    let mut issues = g.issues;
    // firma automatica con certificato dell'archivio / smart card
    let sc = sign_config(store);
    if matches!(sc.source, openfatt_sign::SignSource::Store { .. }) && (sc.auto_all || (sc.auto_pa && e.is_pa())) {
        match sign_einvoice(store, e.id, "") {
            Ok(signed) => e = signed,
            Err(err) => issues.push(openfatt_fatturapa::Issue {
                severity: Severity::Warning,
                code: "GA-FIRMA".into(),
                path: "Firma".into(),
                message: format!("firma automatica non riuscita: {} (firmare da 'Fatture elettr. > Fatture inviate')", err),
            }),
        }
    } else if e.is_pa() {
        issues.push(openfatt_fatturapa::Issue {
            severity: Severity::Warning,
            code: "GA-FIRMA".into(),
            path: "Firma".into(),
            message: "le fatture verso la PA devono essere firmate digitalmente prima dell'invio".into(),
        });
    }
    Ok((e, issues))
}

pub fn document_pdf(store: &Store, doc: &Document) -> Result<Vec<u8>, String> {
    crate::printing::document_pdf(store, doc)
}

pub fn einvoice_pdf(xml: &[u8]) -> Result<Vec<u8>, String> {
    let f = openfatt_fatturapa::parse_bytes(xml).map_err(|e| e.0)?;
    Ok(openfatt_pdf::render(&openfatt_pdf::from_einvoice(&f, 0)))
}

pub fn temp_file(name: &str, data: &[u8]) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("GestionaleAperto");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let p = dir.join(safe_name(name));
    std::fs::write(&p, data).map_err(|e| e.to_string())?;
    Ok(p)
}

/// Apre un file (o un indirizzo web) con il programma predefinito di Windows.
///
/// Si usa direttamente ShellExecute: passare dal prompt dei comandi (`cmd /C start`) era
/// pericoloso, perché caratteri come `&` o `^` nel percorso venivano interpretati come
/// comandi.
pub fn open_path(p: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let file = wide(p.as_os_str());
        let verb = wide(std::ffi::OsStr::new("open"));
        // SAFETY: stringhe UTF-16 terminate da zero che vivono per tutta la chiamata
        unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(p).spawn();
    }
}

/// Apre un file prodotto dal programma (PDF, stampe, esportazioni).
pub fn open_bytes(name: &str, data: &[u8]) -> Result<(), String> {
    let p = temp_file(name, data)?;
    open_path(&p);
    Ok(())
}

/// Tipi di file che si possono aprire direttamente anche se arrivano da fuori (allegati delle
/// fatture ricevute, delle email, documenti allegati): non contengono programmi eseguibili.
const SAFE_TO_OPEN: &[&str] = &[
    "pdf", "xml", "txt", "csv", "jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "docx", "xlsx", "pptx", "odt", "ods", "odp", "rtf", "p7m", "eml",
];

/// Vero se un file con questo nome si può aprire senza rischi.
pub fn safe_to_open(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.trim().to_ascii_lowercase()).unwrap_or_default();
    SAFE_TO_OPEN.contains(&ext.as_str())
}

/// Apre un file che arriva da fuori (allegato di una fattura o di un'email, file allegato
/// all'archivio). Il file viene segnato come "scaricato da Internet", così Windows e Office
/// lo aprono in modalità protetta; i tipi che possono contenere programmi (.exe, .js, .hta,
/// .lnk, .docm...) non vengono aperti: si propone di salvarli per farli controllare.
pub fn open_untrusted(name: &str, data: &[u8]) -> Result<(), String> {
    if !safe_to_open(name) {
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_string()).unwrap_or_else(|| "senza estensione".into());
        let save = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("File potenzialmente pericoloso")
            .set_description(format!(
                "Per sicurezza il file \"{}\" non viene aperto: i file di tipo .{} possono contenere programmi.\n\nSalvarlo in una cartella per farlo controllare dall'antivirus?",
                name, ext
            ))
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();
        if save == rfd::MessageDialogResult::Yes {
            if let Some(p) = rfd::FileDialog::new().set_file_name(safe_name(name)).save_file() {
                std::fs::write(&p, data).map_err(|e| e.to_string())?;
                mark_from_internet(&p);
            }
        }
        return Ok(());
    }
    let p = temp_file(name, data)?;
    mark_from_internet(&p);
    open_path(&p);
    Ok(())
}

/// Segna un file come proveniente da Internet (Mark of the Web): Windows chiede conferma
/// prima di eseguirlo e Office lo apre in visualizzazione protetta.
pub fn mark_from_internet(p: &Path) {
    #[cfg(windows)]
    {
        let mut ads = p.as_os_str().to_os_string();
        ads.push(":Zone.Identifier");
        let _ = std::fs::write(ads, "[ZoneTransfer]\r\nZoneId=3\r\n");
    }
    #[cfg(not(windows))]
    let _ = p;
}

/// Nome di file senza caratteri pericolosi o non validi.
fn safe_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || ".-_".contains(c) { c } else { '_' }).collect();
    // niente nomi fatti solo di punti (".." risalirebbe di cartella) né punti finali
    let s = s.trim_matches('.').to_string();
    if s.is_empty() {
        "file".into()
    } else {
        s.chars().take(150).collect()
    }
}

/// Toglie i file temporanei del programma più vecchi di un giorno (stampe e allegati aperti
/// restano altrimenti nella cartella temporanea con i dati dei clienti).
pub fn clean_temp_files() {
    let dir = std::env::temp_dir().join("GestionaleAperto");
    let Ok(list) = std::fs::read_dir(&dir) else { return };
    let limit = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 3600);
    for e in list.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).map(|t| t < limit).unwrap_or(false);
        if old && e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct InboundSummary {
    pub notifications: usize,
    pub invoices: usize,
    pub duplicates: usize,
    pub errors: Vec<String>,
    pub sdi_address_changed: Option<String>,
}

impl InboundSummary {
    pub fn text(&self) -> String {
        let mut s = format!("{} notifiche acquisite, {} fatture ricevute", self.notifications, self.invoices);
        if self.duplicates > 0 {
            s.push_str(&format!(", {} già presenti", self.duplicates));
        }
        if let Some(a) = &self.sdi_address_changed {
            s.push_str(&format!(". Indirizzo SdI aggiornato a {}", a));
        }
        if !self.errors.is_empty() {
            s.push_str(&format!(". {} errori:\n{}", self.errors.len(), self.errors.join("\n")));
        }
        s
    }
}

/// Acquisisce un singolo file (notifica SdI o fattura ricevuta).
pub fn import_file(store: &Store, file_name: &str, bytes: &[u8], sum: &mut InboundSummary) {
    if naming::is_notification_name(file_name) || openfatt_fatturapa::notify::parse_notification_bytes(bytes).is_ok() {
        let raw = if bytes.first() == Some(&0x30) { openfatt_fatturapa::p7m::extract(bytes).unwrap_or_else(|_| bytes.to_vec()) } else { bytes.to_vec() };
        match openfatt_fatturapa::notify::parse_notification_bytes(&raw) {
            Ok(n) => {
                if n.kind == openfatt_fatturapa::notify::NotificationKind::Metadata {
                    return;
                }
                match store.apply_notification(file_name, &n, &raw) {
                    Ok(Some(_)) => sum.notifications += 1,
                    Ok(None) => sum.duplicates += 1,
                    Err(e) => sum.errors.push(format!("{}: {}", file_name, e)),
                }
            }
            Err(e) => sum.errors.push(format!("{}: {}", file_name, e)),
        }
        return;
    }
    match openfatt_fatturapa::read_any(bytes) {
        Ok((f, xml)) => {
            let sdi_id = String::new();
            let original = if xml != bytes { Some(bytes) } else { None };
            match store.import_incoming(file_name, &xml, original, &f, &sdi_id) {
                Ok(Some(_)) => sum.invoices += 1,
                Ok(None) => sum.duplicates += 1,
                Err(e) => sum.errors.push(format!("{}: {}", file_name, e)),
            }
        }
        Err(e) => sum.errors.push(format!("{}: {}", file_name, e)),
    }
}

/// Elabora i messaggi letti dalla casella PEC.
pub fn process_inbound(store: &Store, msgs: Vec<InboundMessage>) -> InboundSummary {
    let mut sum = InboundSummary::default();
    let mut cfg = pec_config(store);
    for m in msgs {
        if store.mail_already_seen(&m.message_id).unwrap_or(false) {
            continue;
        }
        if m.is_from_sdi() && !m.is_pec_receipt {
            // lo SdI comunica l'indirizzo dedicato rispondendo dal canale assegnato
            let from = m.original_from.to_ascii_lowercase();
            if from.starts_with("sdi") && from != cfg.sdi_address.to_ascii_lowercase() && from != openfatt_pec::SDI_FIRST_ADDRESS {
                cfg.sdi_address = from.clone();
                sum.sdi_address_changed = Some(from);
            }
        }
        for f in &m.notifications {
            import_file(store, &f.file_name, &f.content, &mut sum);
        }
        for f in &m.invoices {
            import_file(store, &f.file_name, &f.content, &mut sum);
        }
        let outcome = if m.is_relevant() { "elaborato" } else { "ignorato" };
        let _ = store.mark_mail_seen(&m.message_id, outcome);
    }
    if sum.sdi_address_changed.is_some() {
        let _ = store.set_setting("pec", &cfg);
    }
    sum
}

/// Risultati dei lavori PEC eseguiti in un thread separato.
pub enum PecJobResult {
    Sent { einvoice_id: i64, result: Result<String, String> },
    Fetched(Result<Vec<InboundMessage>, String>),
    Tested(Result<String, String>),
}

pub fn spawn_send(cfg: PecConfig, password: String, einvoice_id: i64, file_name: String, xml: Vec<u8>, tx: std::sync::mpsc::Sender<PecJobResult>) {
    std::thread::spawn(move || {
        let r = openfatt_pec::send_invoice(&cfg, &password, &file_name, &xml).map_err(|e| e.to_string());
        let _ = tx.send(PecJobResult::Sent { einvoice_id, result: r });
    });
}

pub fn spawn_fetch(cfg: PecConfig, password: String, tx: std::sync::mpsc::Sender<PecJobResult>) {
    std::thread::spawn(move || {
        let r = openfatt_pec::fetch_messages(&cfg, &password, &|_| false).map_err(|e| e.to_string());
        let _ = tx.send(PecJobResult::Fetched(r));
    });
}

pub fn spawn_test(cfg: PecConfig, password: String, tx: std::sync::mpsc::Sender<PecJobResult>) {
    std::thread::spawn(move || {
        let r = openfatt_pec::test_connection(&cfg, &password).map_err(|e| e.to_string());
        let _ = tx.send(PecJobResult::Tested(r));
    });
}

#[cfg(test)]
mod sicurezza {
    use super::*;

    #[test]
    fn allegati_pericolosi_non_si_aprono() {
        for ok in ["fattura.pdf", "FOTO.JPG", "listino.xlsx", "IT01234567890_00001.xml", "contratto.p7m"] {
            assert!(safe_to_open(ok), "{}", ok);
        }
        for bad in ["fattura.pdf.exe", "documento.js", "avviso.hta", "collegamento.lnk", "macro.docm", "foglio.xlsm", "script.ps1", "pagina.html", "archivio.zip", "senza_estensione", "fattura.pdf.", "x.scr", "x.bat", "x.cmd", "x.vbs", "x.msi"] {
            assert!(!safe_to_open(bad), "{}", bad);
        }
    }

    #[test]
    fn nomi_dei_file_temporanei_sicuri() {
        assert_eq!(safe_name("..\\..\\Windows\\evil.dll"), "_.._Windows_evil.dll");
        assert_eq!(safe_name("../../x.pdf"), "_.._x.pdf");
        assert_eq!(safe_name("a&b^c%d.pdf"), "a_b_c_d.pdf");
        assert_eq!(safe_name(".."), "file");
        assert_eq!(safe_name("fattura.pdf..."), "fattura.pdf");
        assert!(safe_name(&"a".repeat(500)).len() <= 150);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;
    use openfatt_core::model::*;

    fn pfx_path() -> String {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/openfatt-sign/tests/data/test-firma.pfx").display().to_string()
    }

    /// Fattura alla PA: avviso di firma, firma con .pfx, payload .p7m, notifica abbinata.
    #[test]
    fn pa_invoice_signature_flow() {
        let s = Store::open_in_memory().unwrap();
        crate::demo::populate(&s).unwrap();
        let pa = s
            .save_party(&Party {
                name: "Comune di Prova".into(),
                vat_number: "00355870221".into(),
                tax_code: "00355870221".into(),
                address: "Piazza Municipio 1".into(),
                zip: "38122".into(),
                city: "Trento".into(),
                province: "TN".into(),
                sdi_code: "UFABCD".into(),
                is_pa: true,
                ..Default::default()
            })
            .unwrap();
        let v22 = s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap();
        let mut d = Document::new(DocKind::Invoice);
        d.party_id = Some(pa.id);
        d.vat_liability = "S".into();
        d.related.push(RelatedDoc { kind: "ORDINE".into(), number: "DET-12".into(), cig: "Z0A1B2C3D4".into(), ..Default::default() });
        d.lines.push(DocLine { description: "Fornitura arredi".into(), qty: dec!(1), unit_price: dec!(500), vat: VatRef::from_code(&v22), ..Default::default() });
        let d = s.save_document(&d).unwrap();
        let d = s.issue_document(d.id).unwrap();

        // senza firma: generata con avviso, non firmata
        let (e, issues) = create_einvoice(&s, d.id).unwrap();
        assert!(e.is_pa());
        assert!(!e.is_signed());
        assert!(issues.iter().any(|i| i.code == "GA-FIRMA"));

        // un .p7m con contenuto diverso viene rifiutato
        let other = openfatt_sign::sign_with_pfx_bytes(b"<altro/>", &std::fs::read(pfx_path()).unwrap(), "prova").unwrap();
        assert!(attach_external_signature(&s, e.id, &other).is_err());

        // firma con certificato .pfx configurato
        s.set_setting("signature", &SignConfig { source: openfatt_sign::SignSource::Pfx { path: pfx_path() }, ..Default::default() }).unwrap();
        assert!(sign_einvoice(&s, e.id, "sbagliata").is_err());
        let signed = sign_einvoice(&s, e.id, "prova").unwrap();
        assert!(signed.is_signed());
        assert!(signed.file_name.ends_with(".xml.p7m"), "{}", signed.file_name);
        let (name, payload) = signed.payload();
        assert!(name.ends_with(".p7m"));
        assert_eq!(openfatt_fatturapa::p7m::extract(payload).unwrap(), signed.xml);
        let v = openfatt_sign::verify(payload).unwrap();
        assert!(v.cades && v.sha256);
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/xsd-samples-firma");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join(name), payload).unwrap();
        std::fs::write(out.join(name.trim_end_matches(".p7m")), &signed.xml).unwrap();

        // la notifica dello SdI riporta il nome .xml.p7m e va abbinata
        s.mark_einvoice_sent(e.id, "test").unwrap();
        let rc = format!(
            r#"<?xml version="1.0"?><types:RicevutaConsegna xmlns:types="http://www.fatturapa.gov.it/sdi/messaggi/v1.0" versione="1.0"><IdentificativoSdI>7</IdentificativoSdI><NomeFile>{}</NomeFile><DataOraRicezione>2026-01-01T10:00:00Z</DataOraRicezione><DataOraConsegna>2026-01-01T10:01:00Z</DataOraConsegna><MessageId>1</MessageId></types:RicevutaConsegna>"#,
            name
        );
        let mut sum = InboundSummary::default();
        import_file(&s, &name.replace(".xml.p7m", "_RC_001.xml"), rc.as_bytes(), &mut sum);
        assert_eq!(sum.notifications, 1, "{:?}", sum.errors);
        assert_eq!(s.einvoice(e.id).unwrap().status, openfatt_store::EInvoiceStatus::Delivered);
        // dopo l'invio la firma non si può più togliere
        assert!(s.remove_signature(e.id).is_err());
    }
}
