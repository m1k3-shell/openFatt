//! Regimi fiscali e ritenute.
//!
//! - Regime forfettario (RF19): nei nuovi documenti di vendita l'aliquota proposta è N2.2
//!   (art. 1 c. 54-89 L. 190/2014), il bollo resta automatico e non si applicano ritenute.
//! - IVA per cassa (RF17, art. 32-bis DL 83/2012): esigibilità differita ("D") nelle vendite
//!   e negli acquisti, con la dicitura obbligatoria; la scissione dei pagamenti prevale.
//! - Contributo Enasarco (RT04) dell'agente di commercio: 8,5% a carico dell'agente
//!   trattenuto dal preponente, entro il massimale provvigionale annuo per preponente
//!   (tabella per anno modificabile nelle impostazioni).
//! - Riepilogo delle ritenute per percipiente (acquisti: compensi pagati a professionisti e
//!   agenti, per la certificazione; vendite: ritenute subite, da ritrovare nelle
//!   certificazioni ricevute). Le ritenute si considerano nell'anno del pagamento.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use openfatt_core::calc;
use openfatt_core::model::*;

use crate::{date_from_sql, dec_from_sql, Result, Store};

/// Dicitura dell'IVA per cassa, obbligatoria in fattura.
pub const CASH_VAT_NOTE: &str = "Operazione con IVA per cassa ai sensi dell'art. 32-bis del D.L. 22 giugno 2012, n. 83";
/// Dicitura dei forfettari sulle parcelle verso sostituti d'imposta.
pub const FLAT_NO_WITHHOLDING_NOTE: &str = "Compenso non soggetto a ritenuta d'acconto ai sensi dell'art. 1, comma 67, della Legge n. 190/2014";

/// Regime dell'azienda che cambia i documenti.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Regime {
    Ordinary,
    /// RF19 (anche RF02 contribuenti minimi: stesse regole sui documenti)
    Flat,
    /// RF17 IVA per cassa
    CashVat,
}

impl Regime {
    pub fn of(code: &str) -> Regime {
        match code.trim() {
            "RF19" | "RF02" => Regime::Flat,
            "RF17" => Regime::CashVat,
            _ => Regime::Ordinary,
        }
    }
}

fn has_text(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn add_line(text: &mut String, line: &str) {
    if has_text(text, line) {
        return;
    }
    if !text.trim().is_empty() {
        text.push('\n');
    }
    text.push_str(line);
}

/// Imposta un documento nuovo (o appena cambiato di soggetto) secondo il regime
/// dell'azienda. Non tocca i documenti emessi.
pub fn apply_regime(regime: Regime, doc: &mut Document, vats: &[VatCode]) {
    let purchase = matches!(doc.kind, DocKind::PurchaseInvoice | DocKind::PurchaseCreditNote);
    if doc.status != DocStatus::Draft || !(doc.kind.is_fiscal() || purchase) {
        return;
    }
    match regime {
        Regime::Ordinary => {}
        Regime::Flat => {
            if !doc.kind.is_sale() || doc.kind == DocKind::SelfInvoice {
                return;
            }
            doc.withholdings.clear();
            doc.vat_liability = "I".into();
            if let Some(v) = flat_vat(vats) {
                for l in doc.lines.iter_mut().filter(|l| l.kind != LineKind::Text) {
                    l.vat = v.clone();
                    l.withholding = false;
                }
            }
            if doc.kind == DocKind::Fee && !doc.party.vat_number.trim().is_empty() {
                add_line(&mut doc.causale, FLAT_NO_WITHHOLDING_NOTE);
            }
        }
        Regime::CashVat => {
            if doc.kind == DocKind::SelfInvoice {
                return;
            }
            // la scissione dei pagamenti (PA) e le operazioni senza IVA restano come sono
            if doc.vat_liability == "S" {
                return;
            }
            // vendite: differita solo verso chi agisce nell'esercizio d'impresa, arte o
            // professione (art. 32-bis c. 1); verso i privati l'imposta resta esigibile subito
            if doc.kind.is_sale() && doc.party.vat_number.trim().is_empty() {
                if doc.vat_liability == "D" {
                    doc.vat_liability = "I".into();
                }
                remove_line(&mut doc.causale, CASH_VAT_NOTE);
                return;
            }
            doc.vat_liability = "D".into();
            if doc.kind.is_sale() {
                add_line(&mut doc.causale, CASH_VAT_NOTE);
            }
        }
    }
}

fn remove_line(text: &mut String, line: &str) {
    if has_text(text, line) {
        *text = text.lines().filter(|l| !l.trim().eq_ignore_ascii_case(line)).collect::<Vec<_>>().join("\n");
    }
}

/// Aliquota dei forfettari: N2.2 (o la prima attiva con natura N2.2).
pub fn flat_vat(vats: &[VatCode]) -> Option<VatRef> {
    vats.iter().filter(|v| v.active).find(|v| v.natura.as_deref() == Some("N2.2")).map(VatRef::from_code)
}

/// Valori Enasarco di un anno.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EnasarcoYear {
    pub year: i32,
    /// Aliquota complessiva (agente + preponente), es. 17.
    pub rate: Decimal,
    /// Parte a carico dell'agente, trattenuta in fattura, es. 8,5.
    pub agent_rate: Decimal,
    /// Massimale provvigionale annuo per preponente: monomandatario e plurimandatario.
    pub cap_single: Decimal,
    pub cap_multi: Decimal,
}

impl Default for EnasarcoYear {
    fn default() -> Self {
        EnasarcoYear { year: 0, rate: Decimal::new(17, 0), agent_rate: Decimal::new(85, 1), cap_single: Decimal::ZERO, cap_multi: Decimal::ZERO }
    }
}

/// Impostazioni Enasarco dell'azienda (agente di commercio).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EnasarcoSettings {
    /// Agente plurimandatario (massimale più basso, causale CU "R").
    pub multi: bool,
    /// Società di capitali: nessun massimale.
    pub no_cap: bool,
    pub years: Vec<EnasarcoYear>,
}

impl Default for EnasarcoSettings {
    fn default() -> Self {
        // valori pubblicati dalla Fondazione Enasarco per il 2025 e il 2026
        EnasarcoSettings {
            multi: false,
            no_cap: false,
            years: vec![
                EnasarcoYear { year: 2025, cap_single: Decimal::new(45085, 0), cap_multi: Decimal::new(30057, 0), ..Default::default() },
                EnasarcoYear { year: 2026, cap_single: Decimal::new(45717, 0), cap_multi: Decimal::new(30478, 0), ..Default::default() },
            ],
        }
    }
}

impl EnasarcoSettings {
    /// Valori dell'anno; se l'anno manca, quelli dell'ultimo anno precedente (e false).
    pub fn year(&self, year: i32) -> (EnasarcoYear, bool) {
        if let Some(y) = self.years.iter().find(|y| y.year == year) {
            return (y.clone(), true);
        }
        let prev = self.years.iter().filter(|y| y.year < year).max_by_key(|y| y.year).or_else(|| self.years.iter().max_by_key(|y| y.year));
        (prev.cloned().unwrap_or_default(), false)
    }

    pub fn cap(&self, year: i32) -> Option<Decimal> {
        if self.no_cap {
            return None;
        }
        let (y, _) = self.year(year);
        Some(if self.multi { y.cap_multi } else { y.cap_single })
    }
}

/// Massimale Enasarco di un preponente in un anno.
#[derive(Debug, Clone, PartialEq)]
pub struct EnasarcoUsage {
    pub year: i32,
    pub cap: Option<Decimal>,
    /// Provvigioni già assoggettate nell'anno (documenti emessi).
    pub used: Decimal,
    /// Ancora assoggettabile (None = senza massimale).
    pub residual: Option<Decimal>,
    /// I valori dell'anno sono stati inseriti (altrimenti si usano quelli dell'anno prima).
    pub year_known: bool,
}

/// Riga del riepilogo delle ritenute: una per ritenuta di ogni documento.
#[derive(Debug, Clone, PartialEq)]
pub struct WithholdingRow {
    pub document_id: i64,
    pub party_id: Option<i64>,
    pub party_name: String,
    pub tax_code: String,
    pub doc_label: String,
    pub date: NaiveDate,
    /// Data del pagamento completo (ultimo incasso/pagamento), se pagato.
    pub paid_on: Option<NaiveDate>,
    /// Compenso (imponibile del documento).
    pub fee: Decimal,
    /// RT01...RT06
    pub kind: String,
    pub reason: String,
    pub rate: Decimal,
    pub base: Decimal,
    pub amount: Decimal,
}

/// Totali per percipiente (codice fiscale o partita IVA).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PercipientTotal {
    pub party_id: Option<i64>,
    pub party_name: String,
    pub tax_code: String,
    pub documents: usize,
    pub fee: Decimal,
    /// Ritenute d'acconto (RT01, RT02)
    pub tax_base: Decimal,
    pub tax: Decimal,
    /// Contributi (RT03-RT06: INPS, Enasarco...)
    pub contributions: Decimal,
    /// Documenti dell'anno non ancora pagati (fuori dal totale)
    pub unpaid: usize,
}

const ENASARCO: &str = "RT04";

impl Store {
    pub fn enasarco_settings(&self) -> Result<EnasarcoSettings> {
        let v: Option<EnasarcoSettings> = self.get_setting("enasarco")?;
        Ok(v.unwrap_or_default())
    }

    pub fn set_enasarco_settings(&self, s: &EnasarcoSettings) -> Result<()> {
        self.set_setting("enasarco", &Some(s.clone()))
    }

    /// Regime dell'azienda (dal regime fiscale nell'anagrafica dell'azienda).
    pub fn regime(&self) -> Regime {
        self.company().map(|c| Regime::of(&c.regime_fiscale)).unwrap_or(Regime::Ordinary)
    }

    /// Provvigioni già assoggettate a Enasarco per il preponente nell'anno, nei documenti
    /// di vendita emessi (escluso `exclude`, il documento in modifica).
    pub fn enasarco_usage(&self, party_id: i64, year: i32, exclude: Option<i64>) -> Result<EnasarcoUsage> {
        let s = self.enasarco_settings()?;
        let (_, known) = s.year(year);
        let mut used = Decimal::ZERO;
        let ids: Vec<i64> = {
            let mut st = self.conn().prepare_cached(
                "SELECT id FROM documents WHERE party_id = ?1 AND status = ?2 AND substr(date, 1, 4) = ?3 AND data LIKE '%\"RT04\"%'",
            )?;
            let rows = st.query_map(rusqlite::params![party_id, DocStatus::Issued.code(), format!("{:04}", year)], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for id in ids {
            if Some(id) == exclude {
                continue;
            }
            let d = self.document(id)?;
            if !d.kind.is_sale() {
                continue;
            }
            let Ok(t) = calc::compute(&d) else { continue };
            let sign = if d.kind.is_credit() { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
            used += t.withholdings.iter().filter(|w| w.kind == ENASARCO).map(|w| w.base).sum::<Decimal>() * sign;
        }
        let cap = s.cap(year);
        Ok(EnasarcoUsage { year, cap, used, residual: cap.map(|c| (c - used).max(Decimal::ZERO)), year_known: known })
    }

    /// Contributo Enasarco da mettere in un documento per il preponente, con il residuo del
    /// massimale come limite.
    pub fn enasarco_withholding(&self, party_id: Option<i64>, date: NaiveDate, exclude: Option<i64>) -> Result<Withholding> {
        let s = self.enasarco_settings()?;
        let (y, _) = s.year(date.year());
        let max_base = match party_id {
            Some(p) => self.enasarco_usage(p, date.year(), exclude)?.residual,
            None => s.cap(date.year()),
        };
        Ok(Withholding { kind: ENASARCO.into(), rate: y.agent_rate, base_percent: Decimal::ONE_HUNDRED, payment_reason: if s.multi { "R".into() } else { "Q".into() }, max_base })
    }

    /// Ritenute proposte per il cliente (profilo nella sua scheda).
    pub fn withholdings_for(&self, profile: &str, party_id: Option<i64>, date: NaiveDate, exclude: Option<i64>) -> Result<Vec<Withholding>> {
        Ok(match profile {
            "rt20" => vec![Withholding::default()],
            "agente" => {
                let multi = self.enasarco_settings()?.multi;
                let reason = if multi { "R" } else { "Q" };
                vec![
                    Withholding { kind: "RT01".into(), rate: Decimal::new(23, 0), base_percent: Decimal::new(50, 0), payment_reason: reason.into(), max_base: None },
                    self.enasarco_withholding(party_id, date, exclude)?,
                ]
            }
            _ => Vec::new(),
        })
    }

    /// Ritenute dei documenti per l'anno del pagamento. `purchases` = compensi pagati
    /// (acquisti), altrimenti ritenute subite (vendite). I documenti dell'anno non ancora
    /// pagati compaiono con `paid_on` vuoto.
    pub fn withholding_rows(&self, year: i32, purchases: bool) -> Result<Vec<WithholdingRow>> {
        let kinds: &[DocKind] = if purchases {
            &[DocKind::PurchaseInvoice, DocKind::PurchaseCreditNote]
        } else {
            &[DocKind::Invoice, DocKind::AccompanyingInvoice, DocKind::CreditNote, DocKind::DebitNote, DocKind::DownPaymentInvoice, DocKind::Fee]
        };
        let marks: Vec<String> = kinds.iter().map(|k| format!("'{}'", k.code())).collect();
        let ids: Vec<i64> = {
            let mut st = self.conn().prepare(&format!(
                "SELECT id FROM documents WHERE status = ?1 AND kind IN ({}) AND date >= ?2 AND date <= ?3 AND json_array_length(data, '$.withholdings') > 0 ORDER BY date, id",
                marks.join(",")
            ))?;
            let rows = st.query_map(rusqlite::params![DocStatus::Issued.code(), format!("{:04}-01-01", year - 1), format!("{:04}-12-31", year)], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        let mut out = Vec::new();
        for id in ids {
            let d = self.document(id)?;
            let Ok(t) = calc::compute(&d) else { continue };
            let paid_on = self.paid_on(id)?;
            let in_year = match paid_on {
                Some(p) => p.year() == year,
                None => d.date.year() == year,
            };
            if !in_year {
                continue;
            }
            let sign = if d.kind.is_credit() { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
            let fee = d.to_eur(t.lines.iter().filter(|l| !l.is_text).map(|l| l.total).sum::<Decimal>()) * sign;
            for w in &t.withholdings {
                out.push(WithholdingRow {
                    document_id: d.id,
                    party_id: d.party_id,
                    party_name: d.party.display_name(),
                    tax_code: if d.party.tax_code.trim().is_empty() { d.party.vat_number.clone() } else { d.party.tax_code.clone() },
                    doc_label: format!("{} {}{}", d.kind.label(), d.number_label(), if d.external_number.is_empty() { String::new() } else { format!(" (n. {})", d.external_number) }),
                    date: d.date,
                    paid_on,
                    fee,
                    kind: w.kind.clone(),
                    reason: w.payment_reason.clone(),
                    rate: w.rate,
                    base: d.to_eur(w.base) * sign,
                    amount: d.to_eur(w.amount) * sign,
                });
            }
        }
        Ok(out)
    }

    /// Data del pagamento completo di un documento (ultima data registrata), se pagato.
    fn paid_on(&self, doc_id: i64) -> Result<Option<NaiveDate>> {
        let mut st = self.conn().prepare_cached("SELECT amount, paid, writeoff, paid_date FROM due_dates WHERE document_id = ?1")?;
        let rows: Vec<(Decimal, Decimal, Decimal, Option<String>)> = st
            .query_map([doc_id], |r| Ok((dec_from_sql(Some(r.get::<_, String>(0)?)), dec_from_sql(Some(r.get::<_, String>(1)?)), dec_from_sql(Some(r.get::<_, String>(2)?)), r.get(3)?)))?
            .collect::<std::result::Result<_, _>>()?;
        if rows.is_empty() {
            return Ok(None);
        }
        if rows.iter().any(|(a, p, w, _)| (a.abs() - p.abs() - w.abs()) > Decimal::new(1, 2)) {
            return Ok(None);
        }
        Ok(rows.iter().filter_map(|(_, _, _, d)| d.as_deref().map(date_from_sql)).max())
    }
}

/// Totali per percipiente delle righe pagate; i documenti non pagati si contano a parte.
pub fn percipient_totals(rows: &[WithholdingRow]) -> Vec<PercipientTotal> {
    let mut map: BTreeMap<String, PercipientTotal> = BTreeMap::new();
    let mut seen: std::collections::HashSet<i64> = Default::default();
    for r in rows {
        let key = if r.tax_code.trim().is_empty() { r.party_name.to_uppercase() } else { r.tax_code.trim().to_uppercase() };
        let t = map.entry(key).or_insert_with(|| PercipientTotal { party_id: r.party_id, party_name: r.party_name.clone(), tax_code: r.tax_code.clone(), ..Default::default() });
        let first = seen.insert(r.document_id);
        if r.paid_on.is_none() {
            if first {
                t.unpaid += 1;
            }
            continue;
        }
        if first {
            t.documents += 1;
            t.fee += r.fee;
        }
        if matches!(r.kind.as_str(), "RT01" | "RT02") {
            t.tax_base += r.base;
            t.tax += r.amount;
        } else {
            t.contributions += r.amount;
        }
    }
    let mut v: Vec<PercipientTotal> = map.into_values().collect();
    v.sort_by(|a, b| a.party_name.to_lowercase().cmp(&b.party_name.to_lowercase()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Allocation, NewPayment};
    use openfatt_core::dec;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }

    fn setup() -> (Store, Party, VatRef) {
        let s = Store::open_in_memory().unwrap();
        let c = s.save_party(&Party { name: "Preponente S.p.A.".into(), vat_number: "12345678903".into(), city: "Trento".into(), sdi_code: "ABCDEFG".into(), ..Default::default() }).unwrap();
        let v = s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap();
        (s, c, VatRef::from_code(&v))
    }

    fn fee_doc(s: &Store, c: &Party, v: &VatRef, date: NaiveDate, amount: Decimal) -> Document {
        let mut doc = Document::new(DocKind::Invoice);
        doc.date = date;
        doc.party_id = Some(c.id);
        doc.lines.push(DocLine { description: "Provvigioni".into(), qty: dec!(1), unit_price: amount, vat: v.clone(), withholding: true, ..Default::default() });
        doc.withholdings = s.withholdings_for("agente", Some(c.id), date, None).unwrap();
        doc
    }

    #[test]
    fn enasarco_entro_il_massimale_del_preponente() {
        let (s, c, v) = setup();
        let u = s.enasarco_usage(c.id, 2026, None).unwrap();
        assert_eq!(u.cap, Some(dec!(45717)));
        assert!(u.year_known);
        // prima fattura: 40.000 di provvigioni, tutta assoggettata
        let a = s.save_document(&fee_doc(&s, &c, &v, d(2026, 3, 31), dec!(40000))).unwrap();
        let a = s.issue_document(a.id).unwrap();
        let t = calc::compute(&a).unwrap();
        let ena = t.withholdings.iter().find(|w| w.kind == "RT04").unwrap();
        assert_eq!(ena.base, dec!(40000));
        assert_eq!(ena.amount, dec!(3400.00));
        // ritenuta d'acconto dell'agente: 23% sul 50%
        let rt = t.withholdings.iter().find(|w| w.kind == "RT01").unwrap();
        assert_eq!(rt.base, dec!(20000.00));
        assert_eq!(rt.amount, dec!(4600.00));
        assert_eq!(rt.payment_reason, "Q");
        // seconda fattura: resta 5.717 di massimale
        let b = fee_doc(&s, &c, &v, d(2026, 6, 30), dec!(10000));
        let w = b.withholdings.iter().find(|w| w.kind == "RT04").unwrap();
        assert_eq!(w.max_base, Some(dec!(5717)));
        let t = calc::compute(&b).unwrap();
        let ena = t.withholdings.iter().find(|w| w.kind == "RT04").unwrap();
        assert_eq!(ena.base, dec!(5717));
        assert_eq!(ena.amount, dec!(485.95));
        let b = s.issue_document(s.save_document(&b).unwrap().id).unwrap();
        // massimale esaurito: contributo zero; la nota di credito lo restituisce
        let u = s.enasarco_usage(c.id, 2026, None).unwrap();
        assert_eq!(u.residual, Some(dec!(0)));
        let mut nc = fee_doc(&s, &c, &v, d(2026, 7, 1), dec!(1000));
        nc.kind = DocKind::CreditNote;
        nc.withholdings.retain(|w| w.kind == "RT04");
        nc.withholdings[0].max_base = None;
        s.issue_document(s.save_document(&nc).unwrap().id).unwrap();
        assert_eq!(s.enasarco_usage(c.id, 2026, None).unwrap().residual, Some(dec!(1000)));
        // escludendo il documento in modifica il suo imponibile torna disponibile
        assert_eq!(s.enasarco_usage(c.id, 2026, Some(b.id)).unwrap().residual, Some(dec!(6717)));
        // anno successivo: si riparte; anno senza valori: quelli dell'ultimo anno noto
        let u = s.enasarco_usage(c.id, 2027, None).unwrap();
        assert_eq!(u.cap, Some(dec!(45717)));
        assert!(!u.year_known);
        // plurimandatario e società di capitali
        let mut st = s.enasarco_settings().unwrap();
        st.multi = true;
        s.set_enasarco_settings(&st).unwrap();
        assert_eq!(s.enasarco_usage(c.id, 2025, None).unwrap().cap, Some(dec!(30057)));
        assert_eq!(s.withholdings_for("agente", Some(c.id), d(2025, 1, 1), None).unwrap()[0].payment_reason, "R");
        st.no_cap = true;
        s.set_enasarco_settings(&st).unwrap();
        assert_eq!(s.enasarco_usage(c.id, 2026, None).unwrap().residual, None);
    }

    #[test]
    fn regimi_forfettario_e_iva_per_cassa() {
        let (s, c, v) = setup();
        let vats = s.vat_codes().unwrap();
        let mut doc = Document::new(DocKind::Fee);
        doc.party = c.clone();
        doc.lines.push(DocLine { description: "Consulenza".into(), qty: dec!(1), unit_price: dec!(500), vat: v.clone(), withholding: true, ..Default::default() });
        doc.withholdings.push(Withholding::default());
        apply_regime(Regime::of("RF19"), &mut doc, &vats);
        assert!(doc.withholdings.is_empty());
        assert_eq!(doc.lines[0].vat.natura.as_deref(), Some("N2.2"));
        assert!(doc.causale.contains("comma 67"));
        let t = calc::compute(&doc).unwrap();
        assert_eq!(t.tax, dec!(0));
        assert_eq!(t.stamp, Some(dec!(2.00)));
        // due volte: la dicitura non si ripete
        apply_regime(Regime::Flat, &mut doc, &vats);
        assert_eq!(doc.causale.matches("comma 67").count(), 1);

        let mut inv = Document::new(DocKind::Invoice);
        inv.party = c.clone();
        inv.lines.push(DocLine { description: "Merce".into(), qty: dec!(1), unit_price: dec!(100), vat: v.clone(), ..Default::default() });
        apply_regime(Regime::of("RF17"), &mut inv, &vats);
        assert_eq!(inv.vat_liability, "D");
        assert!(inv.causale.contains("32-bis"));
        // verso un privato l'IVA resta esigibile subito e la dicitura sparisce
        inv.party = Party { name: "Mario Rossi".into(), tax_code: "RSSMRA80A01L378Y".into(), ..Default::default() };
        apply_regime(Regime::CashVat, &mut inv, &vats);
        assert_eq!(inv.vat_liability, "I");
        assert!(!inv.causale.contains("32-bis"));
        let mut pa = Document::new(DocKind::Invoice);
        pa.vat_liability = "S".into();
        apply_regime(Regime::CashVat, &mut pa, &vats);
        assert_eq!(pa.vat_liability, "S");
        let mut buy = Document::new(DocKind::PurchaseInvoice);
        apply_regime(Regime::CashVat, &mut buy, &vats);
        assert_eq!(buy.vat_liability, "D");
        assert!(buy.causale.is_empty());
        // i documenti emessi non si toccano
        let mut done = Document::new(DocKind::Invoice);
        done.status = DocStatus::Issued;
        apply_regime(Regime::CashVat, &mut done, &vats);
        assert_eq!(done.vat_liability, "I");
    }

    #[test]
    fn iva_per_cassa_anche_da_trasformazioni_e_fatture_ricevute() {
        let (s, c, v) = setup();
        s.set_company(&Company { name: "Azienda".into(), vat_number: "06363391001".into(), regime_fiscale: "RF17".into(), ..Default::default() }).unwrap();
        // DDT -> fattura: la fattura nasce dall'archivio, non dall'editor
        let mut ddt = Document::new(DocKind::DeliveryNote);
        ddt.party_id = Some(c.id);
        ddt.lines.push(DocLine { description: "Merce".into(), qty: dec!(1), unit_price: dec!(100), vat: v.clone(), ..Default::default() });
        let ddt = s.issue_document(s.save_document(&ddt).unwrap().id).unwrap();
        let inv = s.transform(&[ddt.id], DocKind::Invoice).unwrap();
        let inv = s.document(inv.id).unwrap();
        assert_eq!(inv.vat_liability, "D");
        assert!(inv.causale.contains("32-bis"));
        // acquisto registrato già emesso (fattura ricevuta): detrazione al pagamento
        let f = s.save_party(&Party { role: PartyRole::Supplier, name: "Fornitore".into(), vat_number: "01234567890".into(), ..Default::default() }).unwrap();
        let mut buy = Document::new(DocKind::PurchaseInvoice);
        buy.status = DocStatus::Issued;
        buy.party_id = Some(f.id);
        buy.external_number = "77".into();
        buy.lines.push(DocLine { description: "Servizio".into(), qty: dec!(1), unit_price: dec!(50), vat: v.clone(), ..Default::default() });
        let buy = s.save_document(&buy).unwrap();
        assert_eq!(buy.vat_liability, "D");
        // i documenti importati da un altro programma restano come sono
        let mut old = Document::new(DocKind::Invoice);
        old.status = DocStatus::Issued;
        old.legacy_fe = true;
        old.number = Some(99);
        old.party_id = Some(c.id);
        old.lines.push(DocLine { description: "Vecchia".into(), qty: dec!(1), unit_price: dec!(10), vat: v, ..Default::default() });
        assert_eq!(s.save_document(&old).unwrap().vat_liability, "I");
    }

    #[test]
    fn riepilogo_ritenute_per_percipiente_nell_anno_del_pagamento() {
        let s = Store::open_in_memory().unwrap();
        let v = VatRef::from_code(&s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap());
        let prof = s.save_party(&Party { role: PartyRole::Supplier, name: "Studio Rossi".into(), tax_code: "RSSMRA80A01L378Y".into(), ..Default::default() }).unwrap();
        let mk = |date: NaiveDate, amount: Decimal| {
            let mut doc = Document::new(DocKind::PurchaseInvoice);
            doc.date = date;
            doc.external_number = format!("{}", date.month());
            doc.party_id = Some(prof.id);
            doc.lines.push(DocLine { description: "Parcella".into(), qty: dec!(1), unit_price: amount, vat: v.clone(), withholding: true, ..Default::default() });
            doc.withholdings.push(Withholding::default());
            s.issue_document(s.save_document(&doc).unwrap().id).unwrap()
        };
        let dic = mk(d(2025, 12, 15), dec!(1000));
        let feb = mk(d(2026, 2, 10), dec!(500));
        let _nov = mk(d(2026, 11, 20), dec!(300));
        let pay = |doc: &Document, date: NaiveDate| {
            let dues = s.dues_of_document(doc.id).unwrap();
            let allocations = dues.iter().map(|x| Allocation { due_id: x.id, amount: x.amount - x.paid, writeoff: false }).collect();
            s.register_payment(&NewPayment { date, mp_code: "MP05".into(), account: "Banca".into(), reference: String::new(), notes: String::new(), allocations }).unwrap();
        };
        // la parcella di dicembre 2025 pagata a gennaio 2026 vale per il 2026
        pay(&dic, d(2026, 1, 20));
        pay(&feb, d(2026, 3, 1));
        let rows = s.withholding_rows(2026, true).unwrap();
        assert_eq!(rows.len(), 3);
        let tot = percipient_totals(&rows);
        assert_eq!(tot.len(), 1);
        assert_eq!(tot[0].documents, 2);
        assert_eq!(tot[0].fee, dec!(1500));
        assert_eq!(tot[0].tax_base, dec!(1500));
        assert_eq!(tot[0].tax, dec!(300.00));
        assert_eq!(tot[0].unpaid, 1);
        // nel 2025 la parcella di dicembre non c'è: è stata pagata nel 2026
        assert!(s.withholding_rows(2025, true).unwrap().is_empty());
        // le vendite sono un elenco a parte
        assert!(s.withholding_rows(2026, false).unwrap().is_empty());
    }
}
