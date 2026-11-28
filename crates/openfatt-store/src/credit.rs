//! Fido e clienti bloccati.
//!
//! L'esposizione di un cliente è quanto deve ancora pagare: residuo delle scadenze aperte
//! dei documenti emessi meno le note di credito non ancora compensate. Superato il fido, o
//! con il cliente bloccato (insolvente), i nuovi documenti di vendita chiedono conferma.

use chrono::NaiveDate;
use rust_decimal::Decimal;

use openfatt_core::model::Party;
use openfatt_core::money::fmt_eur;

use crate::{Result, Store};

/// Situazione del credito verso un cliente.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CreditStatus {
    /// Da incassare (al netto delle note di credito).
    pub exposure: Decimal,
    /// Parte già scaduta alla data.
    pub overdue: Decimal,
    pub limit: Option<Decimal>,
    pub blocked: bool,
}

impl CreditStatus {
    /// Quanto resta del fido (negativo se superato).
    pub fn available(&self) -> Option<Decimal> {
        self.limit.map(|l| l - self.exposure)
    }
}

impl Store {
    /// Esposizione del cliente alla data, escluso il documento `exclude`.
    pub fn credit_status(&self, p: &Party, today: NaiveDate, exclude: Option<i64>) -> Result<CreditStatus> {
        let mut st = CreditStatus { limit: p.credit_limit, blocked: p.blocked, ..Default::default() };
        if p.id == 0 {
            return Ok(st);
        }
        for receivable in [true, false] {
            for r in self.open_dues_of_party(Some(p.id), &p.display_name(), receivable)? {
                if !r.customer_side() || Some(r.document_id) == exclude {
                    continue;
                }
                st.exposure += r.signed_residual();
                if !r.is_credit() && r.date < today {
                    st.overdue += r.residual();
                }
            }
        }
        Ok(st)
    }

    /// Avviso per un nuovo documento di vendita di `amount` al cliente: bloccato o fido
    /// superato. None se non c'è nulla da segnalare.
    pub fn credit_warning(&self, p: &Party, amount: Decimal, exclude: Option<i64>) -> Result<Option<String>> {
        if !p.blocked && p.credit_limit.is_none() {
            return Ok(None);
        }
        let today = chrono::Local::now().date_naive();
        let st = self.credit_status(p, today, exclude)?;
        Ok(credit_text(p, &st, amount))
    }
}

/// Testo dell'avviso (vedi `Store::credit_warning`).
pub fn credit_text(p: &Party, st: &CreditStatus, amount: Decimal) -> Option<String> {
    if st.blocked {
        let why = if p.block_reason.trim().is_empty() { String::new() } else { format!(": {}", p.block_reason.trim()) };
        return Some(format!("{} è bloccato{} (da incassare € {}, scaduto € {})", p.display_name(), why, fmt_eur(st.exposure), fmt_eur(st.overdue)));
    }
    let limit = st.limit?;
    let after = st.exposure + amount;
    if after > limit {
        return Some(format!(
            "Fido di {} superato: da incassare € {}{} su un fido di € {}{}",
            p.display_name(),
            fmt_eur(st.exposure),
            if amount.is_zero() { String::new() } else { format!(" + € {} di questo documento", fmt_eur(amount)) },
            fmt_eur(limit),
            if st.overdue.is_zero() { String::new() } else { format!(", di cui € {} scaduti", fmt_eur(st.overdue)) }
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;
    use openfatt_core::model::*;

    #[test]
    fn fido_al_netto_delle_note_di_credito_e_blocco() {
        let s = Store::open_in_memory().unwrap();
        let v = VatRef::from_code(&s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap());
        let mut c = s.save_party(&Party { name: "Cliente Rischioso".into(), vat_number: "12345678903".into(), credit_limit: Some(dec!(1000)), ..Default::default() }).unwrap();
        let mk = |kind: DocKind, amount: Decimal, date: NaiveDate| {
            let mut doc = Document::new(kind);
            doc.date = date;
            doc.party_id = Some(c.id);
            doc.lines.push(DocLine { description: "Merce".into(), qty: dec!(1), unit_price: amount, vat: v.clone(), ..Default::default() });
            s.issue_document(s.save_document(&doc).unwrap().id).unwrap()
        };
        let today = chrono::Local::now().date_naive();
        assert_eq!(s.credit_warning(&c, dec!(500), None).unwrap(), None);
        // 610 + 488 = 1.098 da incassare: oltre il fido di 1.000
        let a = mk(DocKind::Invoice, dec!(500), today - chrono::Duration::days(90));
        mk(DocKind::Invoice, dec!(400), today);
        let st = s.credit_status(&c, today, None).unwrap();
        assert_eq!(st.exposure, dec!(1098.00));
        assert!(st.overdue >= dec!(610.00));
        assert!(s.credit_warning(&c, Decimal::ZERO, None).unwrap().unwrap().contains("superato"));
        // escludendo la prima fattura (in modifica) si rientra
        assert_eq!(s.credit_warning(&c, Decimal::ZERO, Some(a.id)).unwrap(), None);
        // la nota di credito riduce l'esposizione
        mk(DocKind::CreditNote, dec!(100), today);
        assert_eq!(s.credit_status(&c, today, None).unwrap().exposure, dec!(976.00));
        assert_eq!(s.credit_warning(&c, Decimal::ZERO, None).unwrap(), None);
        assert!(s.credit_warning(&c, dec!(50), None).unwrap().unwrap().contains("50,00"));
        // cliente bloccato: avviso anche senza fido
        c.credit_limit = None;
        c.blocked = true;
        c.block_reason = "insoluto RiBa di marzo".into();
        let w = s.credit_warning(&c, Decimal::ZERO, None).unwrap().unwrap();
        assert!(w.contains("bloccato: insoluto RiBa di marzo"), "{}", w);
    }
}
