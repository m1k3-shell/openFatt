//! Pacchetto per la conservazione digitale delle fatture elettroniche.
//!
//! La conservazione a norma (DM 17/06/2014, Linee guida AgID sul documento informatico)
//! deve essere svolta da un conservatore; il servizio gratuito dell'Agenzia delle Entrate
//! (portale Fatture e Corrispettivi, previa adesione) conserva le fatture transitate dallo
//! SdI. Il gestionale prepara per ogni anno un pacchetto di versamento da consegnare al
//! conservatore scelto: i file originali (anche firmati), le ricevute dello SdI, un indice
//! XML con i metadati di ogni documento e l'impronta SHA-256 di ogni file.

use std::io::Write;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::einvoices::sha256_hex;
use crate::{now_sql, Direction, Result, Store, StoreError};

#[derive(Debug, Clone, PartialEq)]
pub struct ConservationPackage {
    pub file_name: String,
    pub zip: Vec<u8>,
    pub count: usize,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConservationRecord {
    pub id: i64,
    pub year: i32,
    pub created_at: String,
    pub file_name: String,
    pub count: i64,
    pub hash: String,
    pub status: String,
    pub note: String,
}

/// Chi conserva e chi ne risponde: va nell'indice del pacchetto e nel manuale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ConservationSettings {
    /// Conservatore (servizio dell'Agenzia delle Entrate o conservatore scelto).
    pub conservator: String,
    /// Riferimento del contratto o dell'adesione al servizio.
    pub contract: String,
    /// Responsabile della conservazione (per l'azienda produttrice).
    pub responsible: String,
    /// Eventuale delegato.
    pub delegate: String,
    /// Dove si trovano le copie del pacchetto.
    pub storage: String,
}

/// Esito della verifica di un pacchetto già salvato.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PackageCheck {
    pub files: usize,
    pub ok: usize,
    /// File mancanti o con impronta diversa da quella dell'indice.
    pub problems: Vec<String>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

impl Store {
    /// Prepara il pacchetto ZIP delle fatture elettroniche (emesse e ricevute) dell'anno.
    pub fn conservation_package(&self, year: i32) -> Result<ConservationPackage> {
        let company = self.company()?;
        let cs = self.conservation_settings()?;
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut count = 0usize;
        let mut index = String::new();
        index.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        index.push_str(&format!(
            "<PacchettoVersamento anno=\"{}\" creato=\"{}\" produttore=\"{}\" partitaIva=\"{}{}\">\n",
            year,
            now_sql(),
            esc(&company.display_name()),
            esc(&company.vat_country),
            esc(&company.vat_number)
        ));
        index.push_str(&format!(
            "  <Soggetti>\n    <ResponsabileConservazione>{}</ResponsabileConservazione>\n    <Delegato>{}</Delegato>\n    <Conservatore riferimento=\"{}\">{}</Conservatore>\n  </Soggetti>\n",
            esc(&cs.responsible),
            esc(&cs.delegate),
            esc(&cs.contract),
            esc(&cs.conservator)
        ));
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            let add = |zip: &mut zip::ZipWriter<&mut std::io::Cursor<Vec<u8>>>, name: &str, data: &[u8]| -> Result<String> {
                zip.start_file(name, opt).map_err(|e| StoreError::Invalid(e.to_string()))?;
                zip.write_all(data).map_err(|e| StoreError::Invalid(e.to_string()))?;
                Ok(sha256_hex(data))
            };
            for dir in [Direction::Out, Direction::In] {
                let folder = if dir == Direction::Out { "emesse" } else { "ricevute" };
                for e in self.einvoices(dir, "", Some(year))? {
                    // si conservano le fatture consegnate o ricevute, non le bozze o gli scarti
                    if dir == Direction::Out && !e.status.is_final_ok() {
                        continue;
                    }
                    let (name, data) = e.payload();
                    let path = format!("{}/{}", folder, name);
                    let hash = add(&mut zip, &path, data)?;
                    count += 1;
                    index.push_str(&format!(
                        "  <Documento>\n    <TipologiaDocumentale>Fattura elettronica {}</TipologiaDocumentale>\n    <NomeFile>{}</NomeFile>\n    <Impronta algoritmo=\"SHA-256\">{}</Impronta>\n    <IdSdI>{}</IdSdI>\n    <TipoDocumento>{}</TipoDocumento>\n    <Numero>{}</Numero>\n    <Data>{}</Data>\n    <Controparte partitaIva=\"{}\">{}</Controparte>\n    <Importo>{}</Importo>\n    <Firmato>{}</Firmato>\n",
                        if dir == Direction::Out { "emessa" } else { "ricevuta" },
                        esc(&path),
                        hash,
                        esc(&e.sdi_id),
                        esc(&e.doc_type),
                        esc(&e.doc_number),
                        esc(&e.doc_date),
                        esc(&e.party_vat),
                        esc(&e.party_name),
                        openfatt_core::money::fmt_xml(e.total, 2),
                        e.is_signed()
                    ));
                    // ricevute e notifiche dello SdI legate al file
                    let mut st = self.conn().prepare("SELECT file_name, raw FROM einvoice_events WHERE einvoice_id = ?1 AND raw IS NOT NULL ORDER BY id")?;
                    let events: Vec<(String, Vec<u8>)> = st.query_map([e.id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
                    for (fname, raw) in events {
                        let p = format!("{}/ricevute_sdi/{}", folder, fname);
                        let h = add(&mut zip, &p, &raw)?;
                        index.push_str(&format!("    <Ricevuta nomeFile=\"{}\" impronta=\"{}\"/>\n", esc(&p), h));
                    }
                    index.push_str("  </Documento>\n");
                }
            }
            index.push_str("</PacchettoVersamento>\n");
            if count == 0 {
                return Err(StoreError::Invalid(format!("Nessuna fattura elettronica da conservare per il {}", year)));
            }
            add(&mut zip, "indice.xml", index.as_bytes())?;
            let readme = format!(
                "Pacchetto di versamento delle fatture elettroniche {}\r\n\r\nContiene {} fatture (cartelle emesse e ricevute), le ricevute dello SdI e indice.xml con metadati e impronte SHA-256.\r\nConsegnarlo al conservatore accreditato scelto entro il termine di legge (tre mesi dal termine di presentazione della dichiarazione dei redditi).\r\nLe fatture transitate dallo SdI possono essere conservate gratuitamente anche dal servizio dell'Agenzia delle Entrate, previa adesione sul portale Fatture e Corrispettivi.\r\n",
                year, count
            );
            add(&mut zip, "LEGGIMI.txt", readme.as_bytes())?;
            add(&mut zip, "manuale_conservazione_bozza.txt", conservation_manual(&company.display_name(), &cs, year).as_bytes())?;
            zip.finish().map_err(|e| StoreError::Invalid(e.to_string()))?;
        }
        let zip = buf.into_inner();
        let hash = sha256_hex(&zip);
        let file_name = format!("conservazione_fatture_{}_{}.zip", year, chrono::Local::now().format("%Y%m%d"));
        self.conn().execute(
            "INSERT INTO conservation(year, created_at, file_name, count, hash, status) VALUES (?1, ?2, ?3, ?4, ?5, 'PREPARATO')",
            params![year, now_sql(), file_name, count as i64, hash],
        )?;
        self.audit("conservation", year as i64, "package", &format!("{} documenti, {}", count, hash))?;
        Ok(ConservationPackage { file_name, zip, count, hash })
    }

    pub fn conservation_records(&self) -> Result<Vec<ConservationRecord>> {
        let mut st = self.conn().prepare("SELECT id, year, created_at, file_name, count, hash, status, note FROM conservation ORDER BY id DESC")?;
        let rows = st.query_map([], |r| {
            Ok(ConservationRecord { id: r.get(0)?, year: r.get(1)?, created_at: r.get(2)?, file_name: r.get(3)?, count: r.get(4)?, hash: r.get(5)?, status: r.get(6)?, note: r.get(7)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn conservation_settings(&self) -> Result<ConservationSettings> {
        self.get_setting("conservation_settings")
    }

    pub fn set_conservation_settings(&self, s: &ConservationSettings) -> Result<()> {
        self.set_setting("conservation_settings", s)
    }

    /// Segna il pacchetto come consegnato al conservatore (con il riferimento della ricevuta).
    pub fn mark_conservation_delivered(&self, id: i64, note: &str) -> Result<()> {
        self.conn().execute("UPDATE conservation SET status = 'CONSEGNATO', note = ?1 WHERE id = ?2", params![note, id])?;
        self.audit("conservation", id, "delivered", note)?;
        Ok(())
    }
}

/// Bozza del manuale della conservazione per la parte del produttore: chi fa cosa, quali
/// documenti, con quale periodicità. Va completata e firmata dal responsabile; il manuale del
/// conservatore resta quello del conservatore.
pub fn conservation_manual(company: &str, s: &ConservationSettings, year: i32) -> String {
    let or_dash = |v: &str| if v.trim().is_empty() { "(da indicare)".to_string() } else { v.trim().to_string() };
    format!(
        "MANUALE DELLA CONSERVAZIONE - PARTE DEL PRODUTTORE (BOZZA)\r\n\
         Soggetto produttore: {}\r\n\
         Responsabile della conservazione: {}\r\n\
         Delegato: {}\r\n\
         Conservatore: {} (contratto/adesione: {})\r\n\
         Copie del pacchetto conservate in: {}\r\n\r\n\
         1. Oggetto\r\n\
         Fatture elettroniche emesse e ricevute tramite il Sistema di Interscambio, con le relative ricevute e notifiche.\r\n\r\n\
         2. Formazione e versamento\r\n\
         Ogni anno il produttore prepara con openFatt il pacchetto di versamento (pacchetto {} e successivi): file originali, eventuali buste firmate .p7m, ricevute SdI e indice.xml con i metadati (tipo, numero, data, controparte, importo, identificativo SdI) e l'impronta SHA-256 di ogni file. Il pacchetto si consegna al conservatore entro il termine di legge (tre mesi dal termine di presentazione della dichiarazione dei redditi) e se ne registra la consegna con il riferimento della ricevuta del conservatore.\r\n\r\n\
         3. Controlli\r\n\
         Prima della consegna si verificano le impronte con la funzione 'Verifica pacchetto'. Si conservano le fatture consegnate, non le bozze né quelle scartate dallo SdI.\r\n\r\n\
         4. Esibizione\r\n\
         I documenti si esibiscono attraverso il servizio del conservatore; la copia del pacchetto e l'indice permettono di ritrovarli per numero, data e controparte.\r\n\r\n\
         Riferimenti: DPCM 3/12/2013 e DM 17/06/2014; Linee guida AgID sulla formazione, gestione e conservazione dei documenti informatici.\r\n\
         Documento da completare, approvare e firmare a cura del responsabile della conservazione.\r\n",
        or_dash(company),
        or_dash(&s.responsible),
        or_dash(&s.delegate),
        or_dash(&s.conservator),
        or_dash(&s.contract),
        or_dash(&s.storage),
        year
    )
}

/// Ricontrolla un pacchetto salvato: ogni file elencato nell'indice deve esserci con la
/// stessa impronta SHA-256.
pub fn verify_conservation_package(zip_bytes: &[u8]) -> Result<PackageCheck> {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| StoreError::Invalid(format!("Pacchetto non leggibile: {}", e)))?;
    let mut index = String::new();
    zip.by_name("indice.xml").map_err(|_| StoreError::Invalid("Nel pacchetto manca indice.xml".into()))?.read_to_string(&mut index).map_err(|e| StoreError::Invalid(e.to_string()))?;
    let doc = roxmltree_lite::entries(&index);
    let mut out = PackageCheck::default();
    for (name, hash) in doc {
        out.files += 1;
        let mut data = Vec::new();
        match zip.by_name(&name) {
            Ok(mut f) => {
                f.read_to_end(&mut data).map_err(|e| StoreError::Invalid(e.to_string()))?;
                if sha256_hex(&data) == hash {
                    out.ok += 1;
                } else {
                    out.problems.push(format!("{}: impronta diversa", name));
                }
            }
            Err(_) => out.problems.push(format!("{}: mancante", name)),
        }
    }
    Ok(out)
}

/// Lettura minima dell'indice scritto da `conservation_package`: coppie (file, impronta).
mod roxmltree_lite {
    fn unesc(s: &str) -> String {
        s.replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
    }
    fn between<'a>(s: &'a str, a: &str, b: &str) -> Option<&'a str> {
        let i = s.find(a)? + a.len();
        let j = s[i..].find(b)? + i;
        Some(&s[i..j])
    }
    pub fn entries(index: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for block in index.split("<Documento>").skip(1) {
            let block = block.split("</Documento>").next().unwrap_or("");
            if let (Some(n), Some(h)) = (between(block, "<NomeFile>", "</NomeFile>"), between(block, "<Impronta algoritmo=\"SHA-256\">", "</Impronta>")) {
                out.push((unesc(n), h.to_string()));
            }
            for r in block.split("<Ricevuta ").skip(1) {
                if let (Some(n), Some(h)) = (between(r, "nomeFile=\"", "\""), between(r, "impronta=\"", "\"")) {
                    out.push((unesc(n), h.to_string()));
                }
            }
        }
        out
    }
}
