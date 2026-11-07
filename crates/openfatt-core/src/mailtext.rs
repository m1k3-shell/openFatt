//! Testi dei messaggi email: modelli con segnaposto, invio di documenti, solleciti di
//! pagamento su tre livelli e comunicazioni libere.
//!
//! I segnaposto si scrivono tra parentesi graffe, es. `{cliente}`; quelli sconosciuti
//! restano nel testo così come sono, per accorgersene in anteprima.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::model::{Company, Document};
use crate::money::fmt_eur;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct MailTemplate {
    pub subject: String,
    pub body: String,
}

impl MailTemplate {
    fn new(subject: &str, body: &str) -> Self {
        MailTemplate { subject: subject.into(), body: body.into() }
    }
}

/// Modelli dell'azienda (salvati nelle impostazioni dell'archivio).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MailTemplates {
    pub document: MailTemplate,
    pub reminders: [MailTemplate; 3],
    pub communication: MailTemplate,
    /// Giorni di tolleranza dopo la scadenza prima di proporre il sollecito.
    pub grace_days: u32,
    /// Giorni minimi tra due solleciti allo stesso cliente.
    pub min_days_between: u32,
}

impl Default for MailTemplates {
    fn default() -> Self {
        MailTemplates {
            document: MailTemplate::new(
                "{tipo} n. {numero} del {data} - {azienda}",
                "Gentile {cliente},\n\nin allegato trova {tipo_minuscolo} n. {numero} del {data}, di importo complessivo € {totale}.\n{scadenze}{nota_fe}\nRestiamo a disposizione per qualsiasi chiarimento.\n\nCordiali saluti\n{firma}",
            ),
            reminders: [
                MailTemplate::new(
                    "Promemoria di pagamento - {azienda}",
                    "Gentile {cliente},\n\ndai nostri controlli risultano ancora da saldare i seguenti importi:\n\n{elenco}\nTotale scaduto: € {totale_scaduto}\n\nLe saremmo grati se potesse provvedere al pagamento{coordinate}.\nSe ha già effettuato il pagamento, non tenga conto di questo messaggio e ci scusi per il disturbo.\n\nCordiali saluti\n{firma}",
                ),
                MailTemplate::new(
                    "Secondo sollecito di pagamento - {azienda}",
                    "Gentile {cliente},\n\nnonostante il nostro precedente promemoria, risultano ancora insoluti i seguenti importi:\n\n{elenco}\nTotale scaduto: € {totale_scaduto}\n\nLa preghiamo di provvedere al pagamento entro 10 giorni dal ricevimento di questo messaggio{coordinate}.\nSe il pagamento è già stato eseguito, La preghiamo di inviarcene copia.\n\nCordiali saluti\n{firma}",
                ),
                MailTemplate::new(
                    "Ultimo avviso prima del recupero del credito - {azienda}",
                    "Spett.le {cliente},\n\nmalgrado i nostri solleciti, risultano tuttora insoluti i seguenti importi:\n\n{elenco}\nTotale scaduto: € {totale_scaduto}\n\nCon la presente La invitiamo formalmente a saldare quanto dovuto entro 15 giorni dal ricevimento{coordinate}. In mancanza, ci riserveremo di procedere al recupero del credito e di addebitare gli interessi di mora previsti dal D.Lgs. 231/2002.\n\nDistinti saluti\n{firma}",
                ),
            ],
            communication: MailTemplate::new("Comunicazione da {azienda}", "Gentile {cliente},\n\n\n\nCordiali saluti\n{firma}"),
            grace_days: 5,
            min_days_between: 7,
        }
    }
}

impl MailTemplates {
    /// Modello del sollecito per il livello 1, 2 o 3.
    pub fn reminder(&self, level: u8) -> &MailTemplate {
        &self.reminders[(level.clamp(1, 3) - 1) as usize]
    }
}

pub fn reminder_level_label(level: u8) -> &'static str {
    match level {
        0 | 1 => "Promemoria",
        2 => "Secondo sollecito",
        _ => "Ultimo avviso",
    }
}

/// Livello del prossimo sollecito: i solleciti contano solo se inviati dopo la scadenza
/// più vecchia ancora aperta (un nuovo ritardo riparte dal promemoria).
pub fn next_reminder_level(previous_dates: &[NaiveDate], oldest_open_due: NaiveDate) -> u8 {
    let n = previous_dates.iter().filter(|d| **d >= oldest_open_due).count();
    (n + 1).min(3) as u8
}

/// Segnaposto disponibili, con descrizione, per l'aiuto nell'interfaccia.
pub const COMMON_PLACEHOLDERS: &[(&str, &str)] = &[
    ("{cliente}", "nome del cliente"),
    ("{azienda}", "ragione sociale dell'azienda"),
    ("{firma}", "firma impostata"),
    ("{oggi}", "data di oggi"),
    ("{iban}", "IBAN dell'azienda"),
];
pub const DOCUMENT_PLACEHOLDERS: &[(&str, &str)] = &[
    ("{tipo}", "es. Fattura"),
    ("{tipo_minuscolo}", "es. la fattura"),
    ("{numero}", "numero del documento"),
    ("{data}", "data del documento"),
    ("{totale}", "totale del documento"),
    ("{scadenze}", "elenco delle scadenze di pagamento"),
    ("{nota_fe}", "nota sulla copia di cortesia (solo fatture elettroniche)"),
];
pub const REMINDER_PLACEHOLDERS: &[(&str, &str)] = &[
    ("{elenco}", "documenti scaduti con importi e giorni di ritardo"),
    ("{totale_scaduto}", "somma degli importi scaduti"),
    ("{giorni_ritardo}", "giorni di ritardo della scadenza più vecchia"),
    ("{coordinate}", "«, con bonifico intestato a ..., IBAN ...» se l'IBAN è impostato"),
];

/// Sostituisce i segnaposto `{nome}` con i valori indicati.
pub fn render(text: &str, vars: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) if end > 0 && after[..end].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
                let key = &after[..end];
                match vars.iter().find(|(k, _)| *k == key) {
                    Some((_, v)) => out.push_str(v),
                    None => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Segnaposto comuni a tutti i messaggi.
pub fn common_vars(c: &Company, party_name: &str, signature: &str, today: NaiveDate) -> Vec<(&'static str, String)> {
    let firma = if signature.trim().is_empty() {
        let mut f = vec![c.display_name()];
        if !c.phone.trim().is_empty() {
            f.push(format!("Tel. {}", c.phone.trim()));
        }
        if !c.email.trim().is_empty() {
            f.push(c.email.trim().to_string());
        }
        f.join("\n")
    } else {
        signature.trim_end().to_string()
    };
    vec![
        ("cliente", party_name.trim().to_string()),
        ("azienda", c.display_name()),
        ("firma", firma),
        ("oggi", today.format("%d/%m/%Y").to_string()),
        ("iban", c.iban.trim().to_string()),
    ]
}

fn article(kind_label: &str) -> String {
    // le sigle (DDT) restano maiuscole
    let l = if kind_label.split_whitespace().next().is_some_and(|w| w.len() > 1 && w.chars().all(|c| c.is_ascii_uppercase())) { kind_label.to_string() } else { kind_label.to_lowercase() };
    let ll = l.to_lowercase();
    let feminine = ["fattura", "nota", "parcella", "autofattura", "proforma", "integrazione"].iter().any(|w| ll.starts_with(w));
    let art = if ll.starts_with(['a', 'e', 'i', 'o', 'u']) { "l'" } else if feminine { "la " } else { "il " };
    format!("{}{}", art, l)
}

/// Segnaposto per l'invio di un documento. `total` è il totale calcolato del documento.
pub fn document_vars(doc: &Document, total: Decimal) -> Vec<(&'static str, String)> {
    let kind = doc.kind.label();
    let mut scadenze = String::new();
    let open: Vec<_> = doc.due_dates.iter().filter(|d| !d.is_paid()).collect();
    if doc.kind.is_sale() && !doc.kind.is_credit() && !open.is_empty() {
        if open.len() == 1 {
            scadenze = format!("\nScadenza del pagamento: {} (€ {}).\n", open[0].date.format("%d/%m/%Y"), fmt_eur(open[0].residual()));
        } else {
            scadenze.push_str("\nScadenze dei pagamenti:\n");
            for d in open {
                scadenze.push_str(&format!("- {}: € {}\n", d.date.format("%d/%m/%Y"), fmt_eur(d.residual())));
            }
        }
    }
    let nota_fe = if doc.kind.is_fiscal() && doc.kind.is_sale() {
        "\nIl PDF allegato è una copia di cortesia: la fattura elettronica originale è stata trasmessa tramite il Sistema di Interscambio ed è disponibile nella Sua area riservata del sito dell'Agenzia delle Entrate.\n".to_string()
    } else {
        String::new()
    };
    vec![
        ("tipo", kind.to_string()),
        ("tipo_minuscolo", article(kind)),
        ("numero", doc.number_label()),
        ("data", doc.date.format("%d/%m/%Y").to_string()),
        ("totale", fmt_eur(total)),
        ("scadenze", scadenze),
        ("nota_fe", nota_fe),
    ]
}

/// Una scadenza da sollecitare.
#[derive(Debug, Clone, PartialEq)]
pub struct OverdueItem {
    pub doc_label: String,
    pub due: NaiveDate,
    pub residual: Decimal,
}

/// Segnaposto per un sollecito.
pub fn reminder_vars(items: &[OverdueItem], c: &Company, today: NaiveDate) -> Vec<(&'static str, String)> {
    let mut elenco = String::new();
    for it in items {
        let late = (today - it.due).num_days().max(0);
        elenco.push_str(&format!("- {}, scaduta il {} ({} giorni): € {}\n", it.doc_label, it.due.format("%d/%m/%Y"), late, fmt_eur(it.residual)));
    }
    let total: Decimal = items.iter().map(|i| i.residual).sum();
    let oldest = items.iter().map(|i| i.due).min().unwrap_or(today);
    let coordinate = if c.iban.trim().is_empty() {
        String::new()
    } else {
        let bank = if c.bank_name.trim().is_empty() { String::new() } else { format!(" ({})", c.bank_name.trim()) };
        format!(", con bonifico intestato a {}, IBAN {}{}", c.display_name(), c.iban.trim(), bank)
    };
    vec![
        ("elenco", elenco),
        ("totale_scaduto", fmt_eur(total)),
        ("giorni_ritardo", (today - oldest).num_days().max(0).to_string()),
        ("coordinate", coordinate),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dec;
    use crate::model::{DocKind, DueDate};

    fn d(y: i32, m: u32, g: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, g).unwrap()
    }

    #[test]
    fn segnaposto() {
        let v = vec![("cliente", "Bianchi S.p.A.".to_string()), ("totale", "1.220,00".to_string())];
        assert_eq!(render("Gentile {cliente}, totale € {totale}.", &v), "Gentile Bianchi S.p.A., totale € 1.220,00.");
        // sconosciuti e parentesi non chiuse restano invariati
        assert_eq!(render("{ignoto} {cliente} {non chiuso", &v), "{ignoto} Bianchi S.p.A. {non chiuso");
        assert_eq!(render("a {} b {x y}", &v), "a {} b {x y}");
    }

    #[test]
    fn livelli_di_sollecito() {
        let oldest = d(2026, 9, 30);
        assert_eq!(next_reminder_level(&[], oldest), 1);
        assert_eq!(next_reminder_level(&[d(2026, 10, 10)], oldest), 2);
        assert_eq!(next_reminder_level(&[d(2026, 10, 10), d(2026, 10, 25), d(2026, 11, 2)], oldest), 3);
        // solleciti di un ritardo precedente (già saldato) non contano
        assert_eq!(next_reminder_level(&[d(2026, 3, 1), d(2026, 3, 20)], oldest), 1);
    }

    #[test]
    fn testo_del_sollecito() {
        let c = Company { name: "Rossi Arredamenti S.r.l.".into(), iban: "IT60X0542811101000000123456".into(), ..Default::default() };
        let items = vec![
            OverdueItem { doc_label: "Fattura 12 del 03/09/2026".into(), due: d(2026, 10, 3), residual: dec!(1220) },
            OverdueItem { doc_label: "Fattura 15 del 20/09/2026".into(), due: d(2026, 10, 20), residual: dec!(350.5) },
        ];
        let today = d(2026, 11, 7);
        let mut vars = common_vars(&c, "Bianchi S.p.A.", "", today);
        vars.extend(reminder_vars(&items, &c, today));
        let t = MailTemplates::default();
        let body = render(&t.reminder(1).body, &vars);
        assert!(body.contains("Gentile Bianchi S.p.A."));
        assert!(body.contains("- Fattura 12 del 03/09/2026, scaduta il 03/10/2026 (35 giorni): € 1.220,00"));
        assert!(body.contains("Totale scaduto: € 1.570,50"));
        assert!(body.contains("con bonifico intestato a Rossi Arredamenti S.r.l., IBAN IT60X0542811101000000123456."), "{}", body);
        assert!(!body.contains('{'), "segnaposto non sostituiti: {}", body);
        let last = render(&t.reminder(3).body, &vars);
        assert!(last.contains("D.Lgs. 231/2002"));
        assert_eq!(render(&t.reminder(9).subject, &vars), "Ultimo avviso prima del recupero del credito - Rossi Arredamenti S.r.l.");
    }

    #[test]
    fn testo_invio_documento() {
        let c = Company { name: "Rossi Arredamenti S.r.l.".into(), phone: "0461 123456".into(), ..Default::default() };
        let mut doc = Document { kind: DocKind::Invoice, number: Some(12), date: d(2026, 9, 3), ..Default::default() };
        doc.due_dates = vec![DueDate { date: d(2026, 10, 3), amount: dec!(1220), ..Default::default() }];
        let mut vars = common_vars(&c, "Bianchi S.p.A.", "", d(2026, 11, 7));
        vars.extend(document_vars(&doc, dec!(1220)));
        let t = MailTemplates::default();
        let subj = render(&t.document.subject, &vars);
        assert!(subj.starts_with("Fattura n. 12"), "{}", subj);
        let body = render(&t.document.body, &vars);
        assert!(body.contains("in allegato trova la fattura n. 12"), "{}", body);
        assert!(body.contains("Scadenza del pagamento: 03/10/2026 (€ 1.220,00)"));
        assert!(body.contains("copia di cortesia"));
        assert!(body.contains("Rossi Arredamenti S.r.l.\nTel. 0461 123456"));
        assert!(!body.contains('{'));
        // preventivo: niente nota fattura elettronica né scadenze
        let q = Document { kind: DocKind::Quote, number: Some(4), date: d(2026, 9, 3), ..Default::default() };
        let mut vars = common_vars(&c, "Bianchi", "Mario Rossi", d(2026, 11, 7));
        vars.extend(document_vars(&q, dec!(500)));
        let body = render(&t.document.body, &vars);
        assert!(!body.contains("cortesia"));
        assert!(body.contains("il preventivo n. 4"), "{}", body);
        assert!(body.ends_with("Mario Rossi"));
    }
}
