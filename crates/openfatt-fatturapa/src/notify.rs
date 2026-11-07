//! Lettura dei messaggi del Sistema di Interscambio (schema MessaggiTypes v1.x).

use roxmltree::Node;

use crate::parse::{child, children, text, ParseError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationKind {
    /// RC - Ricevuta di consegna
    Delivered,
    /// NS - Notifica di scarto
    Rejected,
    /// MC - Mancata consegna (la fattura è comunque emessa ed è nel cassetto fiscale)
    NotDelivered,
    /// NE - Esito committente (PA): accettazione o rifiuto
    Outcome,
    /// DT - Decorrenza termini (PA)
    Expired,
    /// AT - Attestazione di avvenuta trasmissione con impossibilità di recapito (PA)
    TransmittedUndeliverable,
    /// MT - File dei metadati (fatture ricevute)
    Metadata,
    /// EC - Esito committente inviato dal destinatario
    RecipientOutcome,
    /// SE - Scarto esito committente
    OutcomeRejected,
}

impl NotificationKind {
    pub fn code(self) -> &'static str {
        match self {
            NotificationKind::Delivered => "RC",
            NotificationKind::Rejected => "NS",
            NotificationKind::NotDelivered => "MC",
            NotificationKind::Outcome => "NE",
            NotificationKind::Expired => "DT",
            NotificationKind::TransmittedUndeliverable => "AT",
            NotificationKind::Metadata => "MT",
            NotificationKind::RecipientOutcome => "EC",
            NotificationKind::OutcomeRejected => "SE",
        }
    }
    pub fn from_code(c: &str) -> Option<Self> {
        Some(match c {
            "RC" => NotificationKind::Delivered,
            "NS" => NotificationKind::Rejected,
            "MC" => NotificationKind::NotDelivered,
            "NE" => NotificationKind::Outcome,
            "DT" => NotificationKind::Expired,
            "AT" => NotificationKind::TransmittedUndeliverable,
            "MT" => NotificationKind::Metadata,
            "EC" => NotificationKind::RecipientOutcome,
            "SE" => NotificationKind::OutcomeRejected,
            _ => return None,
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            NotificationKind::Delivered => "Consegnata",
            NotificationKind::Rejected => "Scartata",
            NotificationKind::NotDelivered => "Mancata consegna (disponibile nel cassetto fiscale)",
            NotificationKind::Outcome => "Esito del committente",
            NotificationKind::Expired => "Decorrenza termini",
            NotificationKind::TransmittedUndeliverable => "Trasmessa, impossibile recapitare",
            NotificationKind::Metadata => "Metadati fattura ricevuta",
            NotificationKind::RecipientOutcome => "Esito committente",
            NotificationKind::OutcomeRejected => "Scarto esito committente",
        }
    }
    fn from_root(name: &str) -> Option<Self> {
        Some(match name {
            "RicevutaConsegna" => NotificationKind::Delivered,
            "NotificaScarto" => NotificationKind::Rejected,
            "NotificaMancataConsegna" => NotificationKind::NotDelivered,
            "NotificaEsito" => NotificationKind::Outcome,
            "NotificaDecorrenzaTermini" => NotificationKind::Expired,
            "AttestazioneTrasmissioneFattura" => NotificationKind::TransmittedUndeliverable,
            "MetadatiInvioFile" => NotificationKind::Metadata,
            "NotificaEsitoCommittente" => NotificationKind::RecipientOutcome,
            "ScartoEsitoCommittente" => NotificationKind::OutcomeRejected,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdiError {
    pub code: String,
    pub description: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub kind: NotificationKind,
    pub sdi_id: String,
    /// Nome del file fattura a cui si riferisce.
    pub file_name: String,
    pub received_at: Option<String>,
    pub delivered_at: Option<String>,
    pub errors: Vec<SdiError>,
    /// Per NE/EC: EC01 accettazione, EC02 rifiuto.
    pub outcome: Option<String>,
    pub outcome_description: Option<String>,
    pub message_id: Option<String>,
    pub note: Option<String>,
    /// Per MT: codice destinatario e formato.
    pub recipient_code: Option<String>,
    pub format: Option<String>,
    pub hash: Option<String>,
}

impl Notification {
    /// Riassunto leggibile, con elenco degli errori di scarto.
    pub fn summary(&self) -> String {
        let mut s = format!("{} - file {} (id SdI {})", self.kind.label(), self.file_name, self.sdi_id);
        if let Some(o) = &self.outcome {
            let o_label = match o.as_str() {
                "EC01" => "accettata",
                "EC02" => "rifiutata",
                _ => o.as_str(),
            };
            s.push_str(&format!(" - esito: {}", o_label));
        }
        for e in &self.errors {
            s.push_str(&format!("\n  {} {}", e.code, e.description));
            if let Some(sg) = &e.suggestion {
                s.push_str(&format!(" ({})", sg));
            }
        }
        if let Some(n) = &self.note {
            s.push_str(&format!("\n  Note: {}", n));
        }
        s
    }
}

fn errors(n: Node) -> Vec<SdiError> {
    match child(n, "ListaErrori") {
        None => Vec::new(),
        Some(le) => children(le, "Errore")
            .map(|e| SdiError {
                code: text(e, "Codice").unwrap_or_default(),
                description: text(e, "Descrizione").unwrap_or_default(),
                suggestion: text(e, "Suggerimento"),
            })
            .collect(),
    }
}

/// Legge una notifica SdI. Restituisce errore se il file non è un messaggio SdI noto.
pub fn parse_notification(xml: &str) -> Result<Notification, ParseError> {
    let opts = roxmltree::ParsingOptions { allow_dtd: false, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(xml.trim_start_matches('\u{feff}'), opts)
        .map_err(|e| ParseError(format!("XML non valido: {}", e)))?;
    let root = doc.root_element();
    let kind = NotificationKind::from_root(root.tag_name().name())
        .ok_or_else(|| ParseError(format!("'{}' non è un messaggio SdI riconosciuto", root.tag_name().name())))?;

    // NotificaEsito contiene l'esito in EsitoCommittente
    let esito = child(root, "EsitoCommittente");
    let outcome = esito.and_then(|e| text(e, "Esito")).or_else(|| text(root, "Esito"));
    let outcome_description = esito.and_then(|e| text(e, "Descrizione")).or_else(|| text(root, "Descrizione"));

    Ok(Notification {
        kind,
        sdi_id: text(root, "IdentificativoSdI").unwrap_or_default(),
        file_name: text(root, "NomeFile").unwrap_or_default(),
        received_at: text(root, "DataOraRicezione"),
        delivered_at: text(root, "DataOraConsegna"),
        errors: errors(root),
        outcome,
        outcome_description,
        message_id: text(root, "MessageId"),
        note: text(root, "Note"),
        recipient_code: text(root, "CodiceDestinatario"),
        format: text(root, "Formato"),
        hash: text(root, "Hash"),
    })
}

pub fn parse_notification_bytes(bytes: &[u8]) -> Result<Notification, ParseError> {
    parse_notification(&crate::parse::decode_xml_bytes(bytes))
}
