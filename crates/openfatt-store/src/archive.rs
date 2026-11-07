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

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

impl Store {
    /// Prepara il pacchetto ZIP delle fatture elettroniche (emesse e ricevute) dell'anno.
    pub fn conservation_package(&self, year: i32) -> Result<ConservationPackage> {
        let company = self.company()?;
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

    /// Segna il pacchetto come consegnato al conservatore (con il riferimento della ricevuta).
    pub fn mark_conservation_delivered(&self, id: i64, note: &str) -> Result<()> {
        self.conn().execute("UPDATE conservation SET status = 'CONSEGNATO', note = ?1 WHERE id = ?2", params![note, id])?;
        self.audit("conservation", id, "delivered", note)?;
        Ok(())
    }
}
