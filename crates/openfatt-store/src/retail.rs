//! Vendite al banco: registrazione con incasso e scarico, esito del registratore, corrispettivi.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;

use openfatt_core::calc;
use openfatt_core::model::*;

use crate::{Result, Store, StoreError};

/// Riepilogo giornaliero dei corrispettivi per aliquota.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RetailDay {
    pub date: Option<NaiveDate>,
    pub count: usize,
    pub gross: Decimal,
    pub cash: Decimal,
    pub electronic: Decimal,
    pub other: Decimal,
    /// (aliquota o natura, imponibile, imposta, lordo)
    pub by_rate: Vec<(String, Decimal, Decimal, Decimal)>,
    pub pending_rt: usize,
    /// Vendite poi fatturate (lordo): scorporate dai corrispettivi del giorno, perché
    /// l'imposta è già nella fattura (FAQ AdE n. 45 del 21/12/2018).
    pub invoiced: Decimal,
}

impl Store {
    /// Registra una vendita al banco: numero progressivo, scarico di magazzino e incassi in
    /// prima nota, in un'unica transazione. Il documento commerciale va poi emesso dal
    /// registratore (`set_retail_outcome`).
    pub fn record_retail_sale(&self, doc: &Document, cash_account: &str, electronic_account: &str) -> Result<Document> {
        let mut doc = doc.clone();
        doc.kind = DocKind::RetailSale;
        doc.prices_include_vat = true;
        doc.status = DocStatus::Issued;
        doc.stamp.mode = StampMode::No;
        doc.payment = None;
        doc.payment_id = None;
        doc.withholdings.clear();
        doc.pension_fund = None;
        let info = doc.retail.clone().ok_or_else(|| StoreError::Invalid("Dati di pagamento mancanti".into()))?;
        if doc.lines.iter().all(|l| l.kind == LineKind::Text) {
            return Err(StoreError::Invalid("Il carrello è vuoto".into()));
        }
        let totals = calc::compute(&doc).map_err(|e| StoreError::Invalid(e.0))?;
        if info.paid() != totals.total {
            return Err(StoreError::Invalid(format!("I pagamenti ({}) non corrispondono al totale ({})", info.paid(), totals.total)));
        }
        if let Some(pid) = doc.party_id {
            if let Some(p) = self.party(pid)? {
                doc.party = p;
            }
        }
        let tx = self.begin()?;
        doc.year = doc.date.year();
        doc.number = Some(self.next_number(DocKind::RetailSale, &doc.series, doc.year)?);
        let saved = self.save_document(&doc)?;
        for p in &info.payments {
            if p.amount.is_zero() || p.method == RetailPayMethod::NotCollected {
                continue;
            }
            let account = match p.method {
                RetailPayMethod::Cash => cash_account,
                _ => electronic_account,
            };
            self.add_cash_entry(&CashEntry {
                id: 0,
                date: saved.date,
                description: format!("Vendita al banco n. {} - {}", saved.number_label(), p.method.label()),
                account: if account.trim().is_empty() { "Cassa".into() } else { account.to_string() },
                amount: p.amount,
                document_id: Some(saved.id),
                due_date_id: None,
                party_id: saved.party_id,
                category: "Corrispettivi".into(),
                ..Default::default()
            })?;
        }
        self.audit("retail", saved.id, "sale", &format!("{} {}", saved.number_label(), totals.total))?;
        tx.commit()?;
        self.document(saved.id)
    }

    /// Registra l'esito del registratore telematico (o gli estremi inseriti a mano).
    pub fn set_retail_outcome(&self, id: i64, status: RtStatus, serial: &str, z: &str, number: &str, message: &str) -> Result<Document> {
        let mut doc = self.document(id)?;
        if doc.kind != DocKind::RetailSale {
            return Err(StoreError::Invalid("Non è una vendita al banco".into()));
        }
        let mut r = doc.retail.clone().unwrap_or_default();
        if matches!(r.rt_status, RtStatus::Printed | RtStatus::Manual) {
            return Err(StoreError::Invalid("Il documento commerciale risulta già emesso".into()));
        }
        r.rt_status = status;
        r.rt_serial = serial.trim().to_string();
        r.z_number = z.trim().to_string();
        r.receipt_number = number.trim().to_string();
        r.rt_message = message.to_string();
        doc.retail = Some(r);
        // aggiornamento diretto: i dati fiscali della vendita non cambiano
        let mut stored = doc.clone();
        stored.due_dates.clear();
        let data = serde_json::to_string(&stored)?;
        let reference = doc.retail.as_ref().map(|r| r.reference()).unwrap_or_default();
        self.conn().execute("UPDATE documents SET data = ?1, external_number = ?2, version = version + 1 WHERE id = ?3", rusqlite::params![data, reference, id])?;
        self.audit("retail", id, "rt", &format!("{:?} {}-{} {}", status, z, number, message))?;
        self.document(id)
    }

    /// Annulla una vendita il cui documento commerciale non è stato emesso.
    pub fn cancel_retail_sale(&self, id: i64) -> Result<()> {
        let doc = self.document(id)?;
        if let Some(r) = &doc.retail {
            if matches!(r.rt_status, RtStatus::Printed | RtStatus::Manual) {
                return Err(StoreError::Invalid("Documento commerciale già emesso: per stornarlo registrare un reso sul registratore".into()));
            }
        }
        let tx = self.begin()?;
        self.conn().execute("DELETE FROM cash_entries WHERE document_id = ?1 AND category = 'Corrispettivi'", [id])?;
        self.cancel_document(id)?;
        tx.commit()?;
        Ok(())
    }

    pub fn retail_sales(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<Document>> {
        let list = self.documents(&crate::DocFilter { kinds: vec![DocKind::RetailSale], from: Some(from), to: Some(to), ..Default::default() })?;
        list.iter().map(|s| self.document(s.id)).collect()
    }

    /// Corrispettivi giornalieri per aliquota nel periodo (vendite non annullate).
    pub fn retail_summary(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<RetailDay>> {
        let mut days: BTreeMap<NaiveDate, (RetailDay, BTreeMap<String, (Decimal, Decimal, Decimal)>)> = BTreeMap::new();
        let invoiced = self.invoiced_receipts_between(from, to)?;
        for d in self.retail_sales(from, to)? {
            if d.status == DocStatus::Cancelled {
                continue;
            }
            let Ok(t) = calc::compute(&d) else { continue };
            let (day, rates) = days.entry(d.date).or_insert_with(|| (RetailDay { date: Some(d.date), ..Default::default() }, BTreeMap::new()));
            day.count += 1;
            if invoiced.contains(&d.id) {
                // l'incasso c'è stato, ma l'imposta va con la fattura
                day.invoiced += t.total;
                if let Some(r) = &d.retail {
                    for p in &r.payments {
                        match p.method {
                            RetailPayMethod::Cash => day.cash += p.amount,
                            RetailPayMethod::Electronic => day.electronic += p.amount,
                            _ => day.other += p.amount,
                        }
                    }
                }
                continue;
            }
            day.gross += t.total;
            if let Some(r) = &d.retail {
                for p in &r.payments {
                    match p.method {
                        RetailPayMethod::Cash => day.cash += p.amount,
                        RetailPayMethod::Electronic => day.electronic += p.amount,
                        _ => day.other += p.amount,
                    }
                }
                if matches!(r.rt_status, RtStatus::Pending | RtStatus::Failed) {
                    day.pending_rt += 1;
                }
            }
            for row in &t.vat_rows {
                let key = match &row.natura {
                    Some(n) => n.clone(),
                    None => format!("{}%", row.rate.normalize()),
                };
                let e = rates.entry(key).or_default();
                e.0 += row.taxable;
                e.1 += row.tax;
                e.2 += row.taxable + row.tax;
            }
        }
        Ok(days
            .into_values()
            .rev()
            .map(|(mut d, rates)| {
                d.by_rate = rates.into_iter().map(|(k, (a, b, c))| (k, a, b, c)).collect();
                d
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;

    fn store_with_product() -> (Store, Product, VatRef) {
        let s = Store::open_in_memory().unwrap();
        let v = s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap();
        let p = s.save_product(&Product { code: "PANE".into(), description: "Pagnotta".into(), price: dec!(2.459), vat_id: Some(v.id), ..Default::default() }).unwrap();
        s.add_manual_stock_move(&StockMove { product_id: p.id, qty: dec!(50), ..Default::default() }).unwrap();
        (s, p, VatRef::from_code(&v))
    }

    fn sale(p: &Product, v: &VatRef, qty: Decimal, gross: Decimal, payments: Vec<RetailPayment>) -> Document {
        let mut d = Document::new(DocKind::RetailSale);
        d.prices_include_vat = true;
        d.lines.push(DocLine { product_id: Some(p.id), code: p.code.clone(), description: p.description.clone(), qty, unit_price: gross, vat: v.clone(), ..Default::default() });
        d.retail = Some(RetailInfo { payments, ..Default::default() });
        d
    }

    #[test]
    fn f15_cash_sale_with_change_and_rt_outcome() {
        // F-15: vendita 18, contanti 20 => resto 2; la ripetizione dell'esito non duplica
        let (s, p, v) = store_with_product();
        let mut d = sale(&p, &v, dec!(6), dec!(3.00), vec![RetailPayment { method: RetailPayMethod::Cash, amount: dec!(18.00) }]);
        if let Some(r) = d.retail.as_mut() {
            r.cash_given = dec!(20);
            r.change = dec!(2);
        }
        let saved = s.record_retail_sale(&d, "Cassa", "Banca").unwrap();
        assert_eq!(saved.number, Some(1));
        assert_eq!(s.stock_of(p.id).unwrap(), dec!(44));
        let cash: Decimal = s.cash_entries(None, None, Some("Cassa")).unwrap().iter().map(|e| e.amount).sum();
        assert_eq!(cash, dec!(18.00));
        let t = calc::compute(&saved).unwrap();
        // 18 IVA inclusa al 22%: imponibile 14,75, imposta 3,25
        assert_eq!(t.taxable, dec!(14.75));
        assert_eq!(t.tax, dec!(3.25));
        let d2 = s.set_retail_outcome(saved.id, RtStatus::Printed, "99MEX000001", "0012", "0034", "ok").unwrap();
        assert_eq!(d2.retail.as_ref().unwrap().reference(), "0012-0034");
        // secondo esito sullo stesso documento: rifiutato (nessuna seconda emissione)
        assert!(s.set_retail_outcome(saved.id, RtStatus::Printed, "x", "1", "2", "").is_err());
        // emesso: non annullabile né modificabile
        assert!(s.cancel_retail_sale(saved.id).is_err());
        assert!(s.document_lock_reason(saved.id).unwrap().is_some());
    }

    #[test]
    fn mixed_payment_must_match_total() {
        let (s, p, v) = store_with_product();
        let bad = sale(&p, &v, dec!(1), dec!(10), vec![RetailPayment { method: RetailPayMethod::Cash, amount: dec!(5) }]);
        assert!(s.record_retail_sale(&bad, "Cassa", "Banca").is_err());
        let ok = sale(
            &p,
            &v,
            dec!(1),
            dec!(10),
            vec![RetailPayment { method: RetailPayMethod::Cash, amount: dec!(4) }, RetailPayment { method: RetailPayMethod::Electronic, amount: dec!(6) }],
        );
        let saved = s.record_retail_sale(&ok, "Cassa", "POS").unwrap();
        let bal = s.account_balances().unwrap();
        assert!(bal.iter().any(|b| b.account == "POS" && b.balance == dec!(6)));
        let today = saved.date;
        let sum = s.retail_summary(today, today).unwrap();
        assert_eq!(sum[0].count, 1);
        assert_eq!(sum[0].cash, dec!(4));
        assert_eq!(sum[0].electronic, dec!(6));
        assert_eq!(sum[0].pending_rt, 1);
        assert_eq!(sum[0].by_rate[0].3, dec!(10));
    }

    #[test]
    fn cancel_unprinted_sale_restores_cash_and_stock() {
        let (s, p, v) = store_with_product();
        let d = sale(&p, &v, dec!(2), dec!(3), vec![RetailPayment { method: RetailPayMethod::Cash, amount: dec!(6) }]);
        let saved = s.record_retail_sale(&d, "Cassa", "Banca").unwrap();
        s.set_retail_outcome(saved.id, RtStatus::Failed, "", "", "", "carta esaurita").unwrap();
        s.cancel_retail_sale(saved.id).unwrap();
        assert_eq!(s.stock_of(p.id).unwrap(), dec!(50));
        assert!(s.cash_entries(None, None, Some("Cassa")).unwrap().is_empty());
        assert!(s.retail_summary(saved.date, saved.date).unwrap().is_empty());
    }

    #[test]
    fn refund_returns_goods_and_money() {
        let (s, p, v) = store_with_product();
        let mut d = sale(&p, &v, dec!(-1), dec!(3), vec![RetailPayment { method: RetailPayMethod::Cash, amount: dec!(-3) }]);
        d.retail.as_mut().unwrap().refund_of = Some(RetailRef { rt_serial: "99MEX000001".into(), z_number: "12".into(), receipt_number: "34".into(), date: None });
        s.record_retail_sale(&d, "Cassa", "Banca").unwrap();
        assert_eq!(s.stock_of(p.id).unwrap(), dec!(51));
        let cash: Decimal = s.cash_entries(None, None, Some("Cassa")).unwrap().iter().map(|e| e.amount).sum();
        assert_eq!(cash, dec!(-3));
    }
}

impl Store {
    /// Vendite al banco del periodo fatturate con fatture emesse. La fattura non può
    /// precedere lo scontrino e si emette al più tardi entro il 15 del mese successivo
    /// (fattura differita): si leggono solo le fatture di quella finestra.
    pub fn invoiced_receipts_between(&self, from: NaiveDate, to: NaiveDate) -> Result<std::collections::HashSet<i64>> {
        let mut st = self.conn().prepare_cached(
            "SELECT json_extract(data, '$.from_receipt_id') FROM documents WHERE kind IN ('FAT', 'FAC') AND status = ?1 AND date >= ?2 AND date <= ?3 AND json_extract(data, '$.from_receipt_id') IS NOT NULL",
        )?;
        let rows = st.query_map(rusqlite::params![DocStatus::Issued.code(), crate::date_to_sql(from), crate::date_to_sql(to + chrono::Duration::days(46))], |r| r.get::<_, i64>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Vendite al banco fatturate: con `issued_only` solo dalle fatture emesse (escono dai
    /// corrispettivi), altrimenti anche dalle bozze (non se ne crea una seconda).
    pub fn invoiced_receipts(&self, issued_only: bool) -> Result<std::collections::HashSet<i64>> {
        let sql = if issued_only {
            "SELECT json_extract(data, '$.from_receipt_id') FROM documents WHERE kind IN ('FAT', 'FAC') AND status = ?1 AND json_extract(data, '$.from_receipt_id') IS NOT NULL"
        } else {
            "SELECT json_extract(data, '$.from_receipt_id') FROM documents WHERE kind IN ('FAT', 'FAC') AND status <> ?1 AND json_extract(data, '$.from_receipt_id') IS NOT NULL"
        };
        let code = if issued_only { DocStatus::Issued.code() } else { DocStatus::Cancelled.code() };
        let mut st = self.conn().prepare_cached(sql)?;
        let rows = st.query_map([code], |r| r.get::<_, i64>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Fattura (in bozza) per una vendita al banco già certificata con documento
    /// commerciale: stesse righe e stessa data, estremi dello scontrino nella fattura
    /// elettronica (AltriDatiGestionali "N.DOC.COMM"), senza scadenze (già incassata) né
    /// scarico di magazzino; la vendita esce dai corrispettivi del giorno.
    pub fn invoice_from_receipt(&self, retail_id: i64, party: &Party) -> Result<Document> {
        let r = self.document(retail_id)?;
        if r.kind != DocKind::RetailSale {
            return Err(StoreError::Invalid("Il documento non è una vendita al banco".into()));
        }
        if r.status != DocStatus::Issued {
            return Err(StoreError::Invalid("La vendita al banco non è registrata o è annullata".into()));
        }
        if party.id == 0 {
            return Err(StoreError::Invalid("Scegliere il cliente da fatturare".into()));
        }
        if self.invoiced_receipts(false)?.contains(&retail_id) {
            return Err(StoreError::Invalid("Questa vendita è già stata fatturata".into()));
        }
        let info = r.retail.clone().unwrap_or_default();
        if info.refund_of.is_some() {
            return Err(StoreError::Invalid("Un reso non si fattura: emettere una nota di credito".into()));
        }
        let mut inv = Document::new(DocKind::Invoice);
        inv.date = r.date;
        inv.party_id = Some(party.id);
        inv.party = party.clone();
        inv.prices_include_vat = r.prices_include_vat;
        inv.discount = r.discount.clone();
        inv.warehouse = r.warehouse.clone();
        inv.lines = r.lines.iter().map(crate::copy_line).collect();
        inv.payment_id = party.default_payment_id;
        let receipt = RetailRef { rt_serial: info.rt_serial.clone(), z_number: info.z_number.clone(), receipt_number: info.receipt_number.clone(), date: Some(r.date) };
        let label = if receipt.receipt_number.trim().is_empty() {
            format!("n. {} del {}", r.number_label(), r.date.format("%d/%m/%Y"))
        } else {
            format!("n. {}-{} del {}", receipt.z_number.trim(), receipt.receipt_number.trim(), r.date.format("%d/%m/%Y"))
        };
        inv.notes = format!("Fattura relativa al documento commerciale {}, già pagato.", label);
        inv.from_receipt = Some(receipt);
        inv.from_receipt_id = Some(r.id);
        self.save_document(&inv)
    }
}

#[cfg(test)]
mod receipt_invoice_tests {
    use super::*;
    use openfatt_core::dec;

    #[test]
    fn scontrino_fatturato_senza_contare_due_volte_iva_magazzino_e_incasso() {
        let s = Store::open_in_memory().unwrap();
        let v = s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap();
        let p = s.save_product(&Product { code: "SEDIA".into(), description: "Sedia".into(), price: dec!(122), vat_id: Some(v.id), manage_stock: true, ..Default::default() }).unwrap();
        s.add_manual_stock_move(&StockMove { product_id: p.id, qty: dec!(10), ..Default::default() }).unwrap();
        let c = s.save_party(&Party { name: "Mario Bianchi".into(), tax_code: "BNCMRA80A01L378B".into(), address: "Via Verdi 3".into(), zip: "38122".into(), city: "Trento".into(), province: "TN".into(), ..Default::default() }).unwrap();
        let mut d = Document::new(DocKind::RetailSale);
        d.prices_include_vat = true;
        d.lines.push(DocLine { product_id: Some(p.id), code: p.code.clone(), description: p.description.clone(), qty: dec!(1), unit_price: dec!(122), vat: VatRef::from_code(&v), ..Default::default() });
        d.retail = Some(RetailInfo { payments: vec![RetailPayment { method: RetailPayMethod::Cash, amount: dec!(122) }], ..Default::default() });
        let sale = s.record_retail_sale(&d, "Cassa", "Banca").unwrap();
        let sale = s.set_retail_outcome(sale.id, RtStatus::Printed, "99MEX000001", "0012", "0034", "ok").unwrap();
        let day = sale.date;
        let period = openfatt_core::vat::Period::containing(day, openfatt_core::vat::Periodicity::Monthly);
        let before = s.vat_period_totals(period).unwrap();
        assert_eq!(before.vat_due, dec!(22.00));
        let stock_before = s.stock_of(p.id).unwrap();
        let cash_before: Decimal = s.cash_entries(None, None, Some("Cassa")).unwrap().iter().map(|e| e.amount).sum();

        let inv = s.invoice_from_receipt(sale.id, &c).unwrap();
        // finché la fattura è in bozza la vendita resta nei corrispettivi
        assert_eq!(s.vat_period_totals(period).unwrap().vat_due, before.vat_due);
        assert_eq!(s.retail_summary(day, day).unwrap()[0].gross, dec!(122.00));
        assert!(s.invoice_from_receipt(sale.id, &c).is_err(), "una sola fattura anche in bozza");
        // la copia della fattura non è legata allo scontrino
        let dup = s.duplicate_document(inv.id, None).unwrap();
        assert!(dup.from_receipt_id.is_none() && dup.from_receipt.is_none());
        s.delete_document(dup.id).unwrap();
        assert_eq!(inv.date, day);
        assert_eq!(inv.from_receipt_id, Some(sale.id));
        let inv = s.issue_document(inv.id).unwrap();
        // stessa imposta: la fattura entra nel registro vendite, la vendita esce dai corrispettivi
        let after = s.vat_period_totals(period).unwrap();
        assert_eq!(after.vat_due, before.vat_due);
        assert_eq!(after.sales_taxable, before.sales_taxable);
        let sum = s.retail_summary(day, day).unwrap();
        assert_eq!(sum[0].invoiced, dec!(122.00));
        assert_eq!(sum[0].gross, dec!(0));
        assert_eq!(sum[0].cash, dec!(122));
        // niente secondo scarico, niente scadenze da incassare, niente doppio incasso
        assert_eq!(s.stock_of(p.id).unwrap(), stock_before);
        assert!(s.dues_of_document(inv.id).unwrap().is_empty());
        let cash_after: Decimal = s.cash_entries(None, None, Some("Cassa")).unwrap().iter().map(|e| e.amount).sum();
        assert_eq!(cash_after, cash_before);
        // una sola fattura per scontrino
        assert!(s.invoice_from_receipt(sale.id, &c).is_err());
        // fattura elettronica: estremi del documento commerciale (FAQ 45)
        let mut company = Company { name: "Negozio".into(), vat_number: "01234567890".into(), regime_fiscale: "RF01".into(), address: "Via Roma".into(), zip: "38122".into(), city: "Trento".into(), province: "TN".into(), country: "IT".into(), vat_country: "IT".into(), ..Default::default() };
        company.transmitter_country = "IT".into();
        let t = calc::compute(&inv).unwrap();
        let f = openfatt_fatturapa::build::build(&company, &inv, &t, &Default::default()).unwrap();
        let xml = openfatt_fatturapa::write::to_xml(&f);
        assert!(xml.contains("<TipoDato>N.DOC.COMM</TipoDato>"), "{}", xml);
        assert!(xml.contains("<RiferimentoTesto>0012-0034</RiferimentoTesto>"));
        assert!(xml.contains(&format!("<RiferimentoData>{}</RiferimentoData>", day.format("%Y-%m-%d"))));
        // annullando la fattura la vendita torna nei corrispettivi
        s.cancel_document(inv.id).unwrap();
        assert_eq!(s.retail_summary(day, day).unwrap()[0].gross, dec!(122.00));
    }
}
