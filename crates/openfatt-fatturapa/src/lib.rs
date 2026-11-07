//! Fatturazione elettronica FatturaPA: costruzione, controlli, XML, lettura,
//! notifiche del Sistema di Interscambio e buste firmate .p7m.
//!
//! Riferimenti pubblici (Agenzia delle Entrate, www.fatturapa.gov.it):
//! - Schema VFPR12 1.2.3 e Specifiche tecniche del formato v1.4;
//! - Elenco dei controlli sul file fattura v2.0;
//! - Specifiche tecniche del Sistema di Interscambio 1.8.x e MessaggiTypes 1.1.

pub mod build;
pub mod model;
pub mod naming;
pub mod notify;
pub mod p7m;
pub mod parse;
pub mod text;
pub mod to_domain;
pub mod validate;
pub mod write;

pub use build::{build, BuildError, BuildOptions, DdtRef};
pub use model::FatturaElettronica;
pub use parse::{parse, parse_bytes, ParseError};
pub use validate::{has_errors, validate, Issue, Severity};
pub use write::to_xml;

use openfatt_core::calc::Totals;
use openfatt_core::model::{Company, Document};

/// Risultato della generazione di una fattura elettronica.
#[derive(Debug, Clone)]
pub struct Generated {
    pub fattura: FatturaElettronica,
    pub xml: String,
    pub issues: Vec<Issue>,
}

impl Generated {
    pub fn is_sendable(&self) -> bool {
        !has_errors(&self.issues)
    }
}

/// Costruisce, controlla e serializza la fattura elettronica di un documento.
pub fn generate(company: &Company, doc: &Document, totals: &Totals, opts: &BuildOptions) -> Result<Generated, BuildError> {
    let fattura = build(company, doc, totals, opts)?;
    let xml = to_xml(&fattura);
    let mut issues = validate(&fattura);
    // controllo di coerenza: l'XML prodotto deve essere rileggibile e identico al modello
    match parse(&xml) {
        Ok(back) => {
            if back.bodies.len() != fattura.bodies.len() || back.bodies[0].linee.len() != fattura.bodies[0].linee.len() {
                issues.push(Issue {
                    severity: Severity::Error,
                    code: "GA-RT".into(),
                    path: "XML".into(),
                    message: "l'XML generato non corrisponde al documento".into(),
                });
            }
        }
        Err(e) => issues.push(Issue { severity: Severity::Error, code: "GA-XML".into(), path: "XML".into(), message: e.0 }),
    }
    Ok(Generated { fattura, xml, issues })
}

/// Legge una fattura da byte qualsiasi: XML semplice, .p7m DER/BER o base64.
pub fn read_any(bytes: &[u8]) -> Result<(FatturaElettronica, Vec<u8>), String> {
    let trimmed = bytes.iter().position(|b| !b.is_ascii_whitespace()).map(|i| &bytes[i..]).unwrap_or(bytes);
    let xml_bytes = if trimmed.starts_with(b"<") || trimmed.starts_with(&[0xEF, 0xBB, 0xBF]) {
        bytes.to_vec()
    } else {
        p7m::extract(bytes).map_err(|e| format!("File firmato non leggibile: {}", e))?
    };
    let f = parse_bytes(&xml_bytes).map_err(|e| e.0)?;
    Ok((f, xml_bytes))
}

#[cfg(test)]
mod tests;
