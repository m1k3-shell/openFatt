//! Flussi di fatturazione fra documenti collegati:
//! - fatturazione differita dei DDT, per un cliente o per tutti (fine mese): una fattura per
//!   cliente e condizione di pagamento, con i riferimenti ai DDT (TD24, DatiDDT);
//! - nota di credito che storna una fattura precisa: estremi in DatiFattureCollegate, riga
//!   "Storno fattura n. ... del ..." e righe stornate (tutte o solo alcune);
//! - fattura di saldo che detrae una o più fatture d'acconto: riferimento alla fattura
//!   d'acconto e importi in negativo, per aliquota.

use std::collections::{BTreeMap, HashMap};

use chrono::NaiveDate;
use rust_decimal::Decimal;

use openfatt_core::calc;
use openfatt_core::model::*;
use openfatt_core::money::round2;

use crate::{DocFilter, Result, Store, StoreError};

/// DDT emesso con merce ancora da fatturare.
#[derive(Debug, Clone)]
pub struct PendingDdt {
    pub id: i64,
    pub number_label: String,
    pub date: NaiveDate,
    pub party_id: i64,
    pub party_name: String,
    pub payment_id: Option<i64>,
    pub payment_name: String,
    /// Causale del trasporto (vendita, conto visione, reso...).
    pub reason: String,
    /// Imponibile ancora da fatturare.
    pub amount: Decimal,
    /// Già fatturato in parte.
    pub partial: bool,
    /// Proposto per la fattura: no per conto visione, riparazione, reso e simili.
    pub suggested: bool,
}

/// Esito della fatturazione differita.
#[derive(Debug, Clone, Default)]
pub struct DeferredRun {
    /// Fatture create (in bozza o emesse).
    pub created: Vec<Document>,
    pub issued: usize,
    /// Gruppi non fatturati, con il motivo.
    pub errors: Vec<String>,
}

/// Fattura d'acconto con l'importo ancora da detrarre.
#[derive(Debug, Clone)]
pub struct DownPayment {
    pub id: i64,
    pub number_label: String,
    pub date: NaiveDate,
    /// Imponibile della fattura d'acconto.
    pub taxable: Decimal,
    /// Totale con IVA.
    pub total: Decimal,
    /// Imponibile già detratto in altre fatture di saldo.
    pub deducted: Decimal,
    pub residual: Decimal,
}

/// Stato di fatturazione di un DDT emesso (colonna "Stato" dell'elenco dei DDT).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DdtBilling {
    /// Nessuna riga ancora fatturata: "Seguirà documento di vendita".
    ToInvoice,
    /// Fatturato solo in parte.
    Partial,
    /// Tutte le righe sono state riprese in fattura.
    Invoiced,
    /// Merce che non si fattura (conto lavorazione, visione, riparazione, reso...).
    NotForSale,
}

/// Fatturazione di un DDT: stato e documenti che ne hanno ripreso le righe.
#[derive(Debug, Clone)]
pub struct DdtBillingInfo {
    pub state: DdtBilling,
    /// (id del documento, "Fatt. 295 del 25/11/2026"), in ordine di data.
    pub documents: Vec<(i64, String)>,
}

impl DdtBillingInfo {
    /// Testo per l'elenco: vuoto per la merce che non si fattura.
    pub fn text(&self) -> String {
        let docs = self.documents.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(", ");
        match self.state {
            DdtBilling::Invoiced => docs,
            DdtBilling::Partial => format!("In parte: {}", docs),
            DdtBilling::ToInvoice => "Seguirà doc. di vendita".into(),
            DdtBilling::NotForSale => String::new(),
        }
    }
}

/// Etichetta breve del documento che ha ripreso le righe di un DDT.
fn billing_label(kind: DocKind, number: Option<i64>, series: &str, date: NaiveDate, draft: bool) -> String {
    let what = match kind {
        DocKind::Invoice | DocKind::AccompanyingInvoice => "Fatt.",
        k => k.label(),
    };
    // il numero di una bozza è provvisorio
    let number = if draft { None } else { number };
    match number {
        Some(n) if series.is_empty() => format!("{} {} del {}", what, n, date.format("%d/%m/%Y")),
        Some(n) => format!("{} {}/{} del {}", what, n, series, date.format("%d/%m/%Y")),
        None => format!("{} in bozza del {}", what, date.format("%d/%m/%Y")),
    }
}

/// La causale del trasporto indica merce che non si fattura. Si confrontano gli inizi
/// delle parole ("c/lavorazione" sì, "trasporto compreso" no).
pub fn not_for_sale(reason: &str) -> bool {
    let low = reason.to_lowercase();
    low.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).any(|w| NOT_SOLD.iter().any(|s| w.starts_with(s)))
}

/// Causali di trasporto che di solito non vanno in fattura.
const NOT_SOLD: &[&str] = &["visione", "riparazion", "lavorazion", "comodato", "reso", "resa", "deposito", "prova", "approvazion", "sostituzion", "garanzia", "noleggio"];

/// Documenti che una nota di credito di vendita può stornare.
pub const CREDITABLE_SALES: &[DocKind] = &[DocKind::Invoice, DocKind::AccompanyingInvoice, DocKind::DebitNote, DocKind::DownPaymentInvoice, DocKind::Fee];

/// Documenti di saldo che possono detrarre le fatture d'acconto.
pub fn can_deduct_down_payments(k: DocKind) -> bool {
    matches!(k, DocKind::Invoice | DocKind::AccompanyingInvoice | DocKind::Fee)
}

/// Documenti stornabili dalla nota di credito di questo tipo.
pub fn creditable_kinds(credit: DocKind) -> &'static [DocKind] {
    match credit {
        DocKind::CreditNote => CREDITABLE_SALES,
        DocKind::PurchaseCreditNote => &[DocKind::PurchaseInvoice],
        _ => &[],
    }
}

/// Numero con cui citare il documento: per gli acquisti quello del fornitore.
pub fn reference_number(d: &Document) -> String {
    if d.kind.is_purchase() && !d.external_number.trim().is_empty() {
        d.external_number.trim().to_string()
    } else {
        d.number_label()
    }
}

/// Fattura d'acconto detratta da questa riga (righe di detrazione della fattura di saldo:
/// senza articolo, collegate al documento d'acconto ma a nessuna sua riga).
pub fn deducted_down_payment(l: &DocLine) -> Option<i64> {
    if l.kind == LineKind::Item && l.product_id.is_none() && l.source_line_id.is_none() {
        l.source_doc_id
    } else {
        None
    }
}

impl Store {
    /// DDT emessi fino al giorno `to` (e dal giorno `from`) con merce non ancora fatturata,
    /// di un cliente o di tutti, ordinati per cliente e data.
    pub fn pending_delivery_notes(&self, from: Option<NaiveDate>, to: NaiveDate, party_id: Option<i64>) -> Result<Vec<PendingDdt>> {
        let f = DocFilter { kinds: vec![DocKind::DeliveryNote], status: Some(DocStatus::Issued), from, to: Some(to), party_id, ..Default::default() };
        let mut out = Vec::new();
        for s in self.documents(&f)? {
            let Some(pid) = s.party_id else { continue };
            let doc = self.document(s.id)?;
            let Ok(t) = calc::compute(&doc) else { continue };
            let mut amount = Decimal::ZERO;
            let mut open = false;
            let mut partial = false;
            for lc in t.lines.iter().filter(|l| !l.is_text && !l.is_generated) {
                let Some(l) = lc.index.and_then(|i| doc.lines.get(i)) else { continue };
                if l.kind != LineKind::Item || l.qty.is_zero() {
                    continue;
                }
                let done = self.fulfilled_qty(l.id)? + l.arrived_qty;
                let left = l.qty - done;
                if done > Decimal::ZERO {
                    partial = true;
                }
                if left > Decimal::ZERO {
                    open = true;
                    amount += round2(lc.total * left / l.qty);
                }
            }
            if !open {
                continue;
            }
            let reason = doc.transport.as_ref().map(|t| t.reason.trim().to_string()).unwrap_or_default();
            let low = reason.to_lowercase();
            out.push(PendingDdt {
                id: doc.id,
                number_label: doc.number_label(),
                date: doc.date,
                party_id: pid,
                party_name: doc.party.display_name(),
                payment_id: doc.payment_id,
                payment_name: doc.payment.as_ref().map(|p| p.name.clone()).unwrap_or_default(),
                suggested: !not_for_sale(&low),
                reason,
                amount,
                partial,
            });
        }
        out.sort_by(|a, b| a.party_name.to_lowercase().cmp(&b.party_name.to_lowercase()).then(a.date.cmp(&b.date)).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Stato di fatturazione dei DDT: fatturati (con il numero della fattura), in parte,
    /// da fatturare, oppure merce che non si fattura (conto lavorazione, visione...).
    pub fn delivery_billing(&self) -> Result<HashMap<i64, DdtBillingInfo>> {
        let code = DocKind::DeliveryNote.code();
        let progress = self.order_progress(&[DocKind::DeliveryNote])?;
        // documenti che hanno ripreso le righe dei DDT
        let mut docs: HashMap<i64, Vec<(i64, String)>> = HashMap::new();
        {
            let mut st = self.conn().prepare(
                "SELECT DISTINCT l.source_doc_id, d.id, d.kind, d.number, d.series, d.date, d.status
                 FROM doc_lines l JOIN documents d ON d.id = l.document_id JOIN documents o ON o.id = l.source_doc_id
                 WHERE o.kind = ?1 AND d.status <> 'ANNULLATO' AND l.source_line_id IS NOT NULL
                 ORDER BY d.date, d.number, d.id",
            )?;
            let rows = st.query_map([code], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?)))?;
            for row in rows {
                let (src, id, kind, number, series, date, status) = row?;
                let kind = DocKind::from_code(&kind).unwrap_or_default();
                let date = crate::date_from_sql(&date);
                let list = docs.entry(src).or_default();
                if !list.iter().any(|(x, _)| *x == id) {
                    list.push((id, billing_label(kind, number, &series, date, status == DocStatus::Draft.code())));
                }
            }
        }
        let mut out = HashMap::new();
        let mut st = self.conn().prepare("SELECT id, COALESCE(json_extract(data, '$.transport.reason'), '') FROM documents WHERE kind = ?1 AND status <> 'BOZZA'")?;
        let rows = st.query_map([code], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (id, reason) = row?;
            let p = progress.get(&id);
            let documents = docs.remove(&id).unwrap_or_default();
            let state = match p {
                Some(p) if p.all && p.any => DdtBilling::Invoiced,
                Some(p) if p.any => DdtBilling::Partial,
                _ if !documents.is_empty() => DdtBilling::Partial,
                _ if not_for_sale(&reason) => DdtBilling::NotForSale,
                _ => DdtBilling::ToInvoice,
            };
            out.insert(id, DdtBillingInfo { state, documents });
        }
        Ok(out)
    }

    /// Fatturazione differita: una fattura per cliente e condizione di pagamento con i DDT
    /// indicati, datata `date`. Con `issue` le fatture si emettono subito (numero progressivo).
    /// Un gruppo che non si può fatturare non ferma gli altri: il motivo va negli errori.
    pub fn deferred_invoices(&self, ddt_ids: &[i64], date: NaiveDate, issue: bool) -> Result<DeferredRun> {
        if ddt_ids.is_empty() {
            return Err(StoreError::Invalid("Nessun DDT selezionato".into()));
        }
        // gruppi per cliente e pagamento, nell'ordine dei nomi
        let mut groups: BTreeMap<(String, i64, i64), Vec<(NaiveDate, i64)>> = BTreeMap::new();
        for id in ddt_ids {
            let d = self.document(*id)?;
            if d.kind != DocKind::DeliveryNote {
                return Err(StoreError::Invalid(format!("{} non è un DDT", d.title())));
            }
            let Some(pid) = d.party_id else {
                return Err(StoreError::Invalid(format!("{} non ha il cliente", d.title())));
            };
            groups.entry((d.party.display_name().to_lowercase(), pid, d.payment_id.unwrap_or(0))).or_default().push((d.date, d.id));
        }
        let mut run = DeferredRun::default();
        for ((_, _, _), mut ids) in groups {
            ids.sort();
            let last = ids.iter().map(|(d, _)| *d).max().unwrap_or(date);
            let ids: Vec<i64> = ids.into_iter().map(|(_, id)| id).collect();
            let name = self.document(ids[0]).map(|d| d.party.display_name()).unwrap_or_default();
            if last > date {
                run.errors.push(format!("{}: la fattura non può avere una data anteriore al DDT del {}", name, last.format("%d/%m/%Y")));
                continue;
            }
            let made = self.transform(&ids, DocKind::Invoice).and_then(|mut inv| {
                inv.date = date;
                self.save_document(&inv)
            });
            let inv = match made {
                Ok(inv) => inv,
                Err(e) => {
                    run.errors.push(format!("{}: {}", name, e));
                    continue;
                }
            };
            if issue {
                match self.issue_document(inv.id) {
                    Ok(done) => {
                        run.issued += 1;
                        run.created.push(done);
                    }
                    Err(e) => {
                        run.errors.push(format!("{}: fattura creata in bozza ma non emessa: {}", name, e));
                        run.created.push(inv);
                    }
                }
            } else {
                run.created.push(inv);
            }
        }
        Ok(run)
    }

    /// Aggiunge alla nota di credito lo storno di una fattura: riferimento alla fattura
    /// (DatiFattureCollegate), riga "Storno fattura n. ... del ..." e le righe indicate
    /// (`lines` = indici delle righe della fattura; None = tutte, vuoto = solo il riferimento).
    /// Se la nota non ha ancora il cliente, lo prende dalla fattura.
    pub fn credit_invoice(&self, doc: &mut Document, invoice_id: i64, lines: Option<&[usize]>) -> Result<()> {
        let inv = self.document(invoice_id)?;
        self.append_credit_source(doc, &inv, lines)
    }

    pub(crate) fn append_credit_source(&self, doc: &mut Document, inv: &Document, lines: Option<&[usize]>) -> Result<()> {
        if !creditable_kinds(doc.kind).contains(&inv.kind) {
            return Err(StoreError::Invalid(format!("{} non si può stornare con {}", inv.kind.label(), doc.kind.label().to_lowercase())));
        }
        if inv.status != DocStatus::Issued {
            return Err(StoreError::Invalid(format!("{} non è emessa", inv.title())));
        }
        match doc.party_id {
            Some(p) if Some(p) != inv.party_id => {
                return Err(StoreError::Invalid(format!("{} è intestata a {}, non a {}", inv.title(), inv.party.display_name(), doc.party.display_name())));
            }
            Some(_) => {}
            None => {
                doc.party_id = inv.party_id;
                doc.party = inv.party.clone();
                doc.payment_id = inv.payment_id;
                doc.payment = inv.payment.clone();
            }
        }
        let has_items = doc.lines.iter().any(|l| l.kind == LineKind::Item);
        if !has_items {
            // stessa impostazione della fattura stornata
            doc.prices_include_vat = inv.prices_include_vat;
            doc.currency = inv.currency.clone();
            doc.exchange_rate = inv.exchange_rate;
            doc.vat_liability = inv.vat_liability.clone();
            doc.pension_fund = inv.pension_fund.clone();
            doc.withholdings = inv.withholdings.clone();
            doc.discount = inv.discount.clone();
            if doc.agent_id.is_none() {
                doc.agent_id = inv.agent_id;
                doc.commission_rate = inv.commission_rate;
            }
        } else if doc.currency_code() != inv.currency_code() {
            return Err(StoreError::Invalid(format!("{} è in {}, la nota di credito in {}", inv.title(), inv.currency_code(), doc.currency_code())));
        }
        let number = reference_number(inv);
        let already = doc.related.iter().any(|r| r.kind == "FATTURA" && r.number.trim() == number && r.date == Some(inv.date));
        if !already {
            // un riferimento vuoto (aggiunto a mano e non compilato) si riusa
            if let Some(r) = doc.related.iter_mut().find(|r| r.kind == "FATTURA" && r.number.trim().is_empty()) {
                r.number = number.clone();
                r.date = Some(inv.date);
            } else {
                doc.related.push(RelatedDoc { kind: "FATTURA".into(), number: number.clone(), date: Some(inv.date), ..Default::default() });
            }
        }
        let label = if inv.kind == DocKind::PurchaseInvoice { "fattura".to_string() } else { inv.kind.label().to_lowercase() };
        let picked: Vec<usize> = match lines {
            None => (0..inv.lines.len()).collect(),
            Some(idx) => idx.iter().copied().filter(|i| *i < inv.lines.len()).collect(),
        };
        let all_items = inv.lines.iter().enumerate().filter(|(_, l)| l.kind == LineKind::Item).all(|(i, _)| picked.contains(&i));
        if !picked.is_empty() {
            doc.lines.push(DocLine {
                kind: LineKind::Text,
                description: format!("Storno {} n. {} del {}", label, number, inv.date.format("%d/%m/%Y")),
                source_doc_id: Some(inv.id),
                ..Default::default()
            });
            for i in &picked {
                let l = &inv.lines[*i];
                let mut nl = l.clone();
                nl.id = 0;
                nl.source_doc_id = Some(inv.id);
                nl.source_line_id = if l.kind == LineKind::Item { Some(l.id) } else { None };
                nl.arrived_qty = Decimal::ZERO;
                nl.not_incoming = false;
                if l.kind == LineKind::Item && doc.prices_include_vat != inv.prices_include_vat {
                    let m = Decimal::ONE + l.vat.rate / Decimal::ONE_HUNDRED;
                    nl.unit_price = openfatt_core::money::round_dp(if doc.prices_include_vat { l.unit_price * m } else { l.unit_price / m }, 6);
                }
                doc.lines.push(nl);
            }
        }
        let what = if picked.is_empty() {
            format!("Rettifica della {} n. {} del {}", label, number, inv.date.format("%d/%m/%Y"))
        } else {
            format!("Storno {} della {} n. {} del {}", if all_items { "totale" } else { "parziale" }, label, number, inv.date.format("%d/%m/%Y"))
        };
        if doc.causale.trim().is_empty() {
            doc.causale = what;
        } else if !doc.causale.contains(&number) {
            doc.causale = format!("{}; {}", doc.causale.trim(), what);
        }
        Ok(())
    }

    /// Fatture emesse del soggetto che la nota di credito può stornare (più recenti prima).
    pub fn creditable_documents(&self, credit: DocKind, party_id: Option<i64>, search: &str) -> Result<Vec<crate::DocSummary>> {
        let f = DocFilter { kinds: creditable_kinds(credit).to_vec(), status: Some(DocStatus::Issued), party_id, search: search.to_string(), limit: Some(200), ..Default::default() };
        self.documents(&f)
    }

    /// Fatture d'acconto emesse del cliente con un importo ancora da detrarre. `exclude_doc`
    /// è la fattura di saldo in lavorazione (le sue detrazioni non contano).
    pub fn open_down_payments(&self, party_id: i64, exclude_doc: i64) -> Result<Vec<DownPayment>> {
        let f = DocFilter { kinds: vec![DocKind::DownPaymentInvoice], status: Some(DocStatus::Issued), party_id: Some(party_id), ..Default::default() };
        let mut out = Vec::new();
        for s in self.documents(&f)? {
            let dp = self.document(s.id)?;
            let Ok(t) = calc::compute(&dp) else { continue };
            let taxable: Decimal = t.lines.iter().filter(|l| !l.is_text).map(|l| l.total).sum();
            let deducted = self.deducted_amount(dp.id, exclude_doc)?;
            let residual = taxable - deducted;
            if residual <= Decimal::ZERO {
                continue;
            }
            out.push(DownPayment { id: dp.id, number_label: dp.number_label(), date: dp.date, taxable, total: t.total, deducted, residual });
        }
        out.sort_by(|a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Imponibile della fattura d'acconto già detratto da altre fatture non annullate.
    pub fn deducted_amount(&self, down_payment_id: i64, exclude_doc: i64) -> Result<Decimal> {
        let mut st = self.conn().prepare(
            "SELECT DISTINCT l.document_id FROM doc_lines l JOIN documents d ON d.id = l.document_id
             WHERE l.source_doc_id = ?1 AND l.source_line_id IS NULL AND d.status <> 'ANNULLATO' AND d.id <> ?2 AND d.kind <> 'FAA'",
        )?;
        let ids: Vec<i64> = st.query_map([down_payment_id, exclude_doc], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
        let mut total = Decimal::ZERO;
        for id in ids {
            let d = self.document(id)?;
            let Ok(t) = calc::compute(&d) else { continue };
            let sign = if d.kind.is_credit() { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
            for lc in t.lines.iter().filter(|l| !l.is_text) {
                if lc.index.and_then(|i| d.lines.get(i)).and_then(deducted_down_payment) == Some(down_payment_id) {
                    total -= lc.total * sign;
                }
            }
        }
        Ok(total)
    }

    /// Detrae dalla fattura di saldo le fatture d'acconto indicate: riferimento alla fattura
    /// d'acconto (DatiFattureCollegate) e, per ogni aliquota, una riga con l'importo in
    /// negativo ("Acconto fattura n. ... del ..."). Si detrae quanto resta di ogni acconto.
    pub fn deduct_down_payments(&self, doc: &mut Document, ids: &[i64]) -> Result<()> {
        if !can_deduct_down_payments(doc.kind) {
            return Err(StoreError::Invalid(format!("Gli acconti si detraggono da una fattura, non da {}", doc.kind.label().to_lowercase())));
        }
        for id in ids {
            let dp = self.document(*id)?;
            if dp.kind != DocKind::DownPaymentInvoice || dp.status != DocStatus::Issued {
                return Err(StoreError::Invalid(format!("{} non è una fattura d'acconto emessa", dp.title())));
            }
            match doc.party_id {
                Some(p) if Some(p) != dp.party_id => {
                    return Err(StoreError::Invalid(format!("{} è intestata a {}", dp.title(), dp.party.display_name())));
                }
                Some(_) => {}
                None => {
                    doc.party_id = dp.party_id;
                    doc.party = dp.party.clone();
                    doc.payment_id = dp.payment_id;
                    doc.payment = dp.payment.clone();
                }
            }
            if doc.lines.iter().any(|l| deducted_down_payment(l) == Some(dp.id)) {
                return Err(StoreError::Invalid(format!("{} è già detratta in questo documento", dp.title())));
            }
            if dp.currency_code() != doc.currency_code() {
                return Err(StoreError::Invalid(format!("{} è in {}, il documento in {}", dp.title(), dp.currency_code(), doc.currency_code())));
            }
            let t = calc::compute(&dp).map_err(|e| StoreError::Invalid(format!("{}: {}", dp.title(), e.0)))?;
            // imponibile per aliquota (con ritenuta e cassa come nelle righe dell'acconto)
            let mut groups: Vec<(VatRef, bool, bool, Decimal)> = Vec::new();
            for lc in t.lines.iter().filter(|l| !l.is_text) {
                match groups.iter_mut().find(|g| g.0 == lc.vat && g.1 == lc.withholding && g.2 == lc.pension_fund) {
                    Some(g) => g.3 += lc.total,
                    None => groups.push((lc.vat.clone(), lc.withholding, lc.pension_fund, lc.total)),
                }
            }
            let taxable: Decimal = groups.iter().map(|g| g.3).sum();
            let deducted = self.deducted_amount(dp.id, doc.id)?;
            let residual = taxable - deducted;
            if residual <= Decimal::ZERO {
                return Err(StoreError::Invalid(format!("{} è già stata detratta per intero", dp.title())));
            }
            // detrazione parziale già fatta altrove: si riduce in proporzione su ogni aliquota
            if residual != taxable && !taxable.is_zero() {
                let mut left = residual;
                let n = groups.len();
                for (i, g) in groups.iter_mut().enumerate() {
                    g.3 = if i + 1 == n { left } else { round2(g.3 * residual / taxable) };
                    left -= g.3;
                }
            }
            let number = dp.number_label();
            let date_s = dp.date.format("%d/%m/%Y").to_string();
            if !doc.related.iter().any(|r| r.kind == "FATTURA" && r.number.trim() == number && r.date == Some(dp.date)) {
                doc.related.push(RelatedDoc { kind: "FATTURA".into(), number: number.clone(), date: Some(dp.date), ..Default::default() });
            }
            let multi = groups.len() > 1;
            for (vat, withholding, pension_fund, amount) in groups {
                if amount.is_zero() {
                    continue;
                }
                let unit = if doc.prices_include_vat { round2(amount + amount * vat.rate / Decimal::ONE_HUNDRED) } else { amount };
                let mut description = format!("Acconto fattura n. {} del {}", number, date_s);
                if multi {
                    description.push_str(&if vat.rate.is_zero() { format!(" ({})", vat.natura.clone().unwrap_or_default()) } else { format!(" (IVA {}%)", vat.rate.normalize()) });
                }
                doc.lines.push(DocLine {
                    kind: LineKind::Item,
                    description,
                    qty: Decimal::ONE,
                    unit_price: -unit,
                    vat,
                    withholding,
                    pension_fund,
                    source_doc_id: Some(dp.id),
                    source_line_id: None,
                    ..Default::default()
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }

    fn setup() -> (Store, Party, Party, Product, VatRef) {
        let s = Store::open_in_memory().unwrap();
        let a = s.save_party(&Party { name: "Alfa S.r.l.".into(), vat_number: "12345678903".into(), city: "Trento".into(), sdi_code: "ABCDEFG".into(), ..Default::default() }).unwrap();
        let b = s.save_party(&Party { name: "Beta S.n.c.".into(), vat_number: "01234567890".into(), city: "Roma".into(), sdi_code: "ABCDEFG".into(), ..Default::default() }).unwrap();
        let v = s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap();
        let p = s.save_product(&Product { code: "A1".into(), description: "Articolo uno".into(), price: dec!(10), vat_id: Some(v.id), ..Default::default() }).unwrap();
        (s, a, b, p, VatRef::from_code(&v))
    }

    fn line(p: &Product, qty: Decimal, vat: &VatRef) -> DocLine {
        DocLine { product_id: Some(p.id), code: p.code.clone(), description: p.description.clone(), qty, unit_price: p.price, vat: vat.clone(), ..Default::default() }
    }

    fn issued(s: &Store, kind: DocKind, party: &Party, date: NaiveDate, lines: Vec<DocLine>, reason: &str) -> Document {
        let mut doc = Document::new(kind);
        doc.date = date;
        doc.party_id = Some(party.id);
        doc.lines = lines;
        if kind.has_transport() {
            doc.transport = Some(Transport { reason: reason.into(), ..Default::default() });
        }
        let doc = s.save_document(&doc).unwrap();
        s.issue_document(doc.id).unwrap()
    }

    #[test]
    fn fine_mese_una_fattura_per_cliente() {
        let (s, a, b, p, v) = setup();
        let a1 = issued(&s, DocKind::DeliveryNote, &a, d(2026, 10, 3), vec![line(&p, dec!(2), &v)], "Vendita");
        let a2 = issued(&s, DocKind::DeliveryNote, &a, d(2026, 10, 20), vec![line(&p, dec!(1), &v)], "Vendita");
        let b1 = issued(&s, DocKind::DeliveryNote, &b, d(2026, 10, 9), vec![line(&p, dec!(5), &v)], "");
        let vis = issued(&s, DocKind::DeliveryNote, &b, d(2026, 10, 10), vec![line(&p, dec!(1), &v)], "Conto visione");
        // DDT di novembre: fuori dal periodo
        issued(&s, DocKind::DeliveryNote, &a, d(2026, 11, 2), vec![line(&p, dec!(1), &v)], "Vendita");

        let pend = s.pending_delivery_notes(Some(d(2026, 10, 1)), d(2026, 10, 31), None).unwrap();
        assert_eq!(pend.iter().map(|x| x.id).collect::<Vec<_>>(), vec![a1.id, a2.id, b1.id, vis.id]);
        assert_eq!(pend[0].amount, dec!(20));
        assert!(!pend.iter().find(|x| x.id == vis.id).unwrap().suggested);

        let ids: Vec<i64> = pend.iter().filter(|x| x.suggested).map(|x| x.id).collect();
        let run = s.deferred_invoices(&ids, d(2026, 10, 31), true).unwrap();
        assert!(run.errors.is_empty(), "{:?}", run.errors);
        assert_eq!(run.created.len(), 2);
        assert_eq!(run.issued, 2);
        let fa = &run.created[0];
        assert_eq!(fa.party_id, Some(a.id));
        assert_eq!(fa.date, d(2026, 10, 31));
        assert_eq!(fa.status, DocStatus::Issued);
        assert_eq!(fa.fe_type, "TD24");
        assert_eq!(s.ddt_sources(fa).unwrap().len(), 2);
        assert_eq!(calc::compute(fa).unwrap().taxable, dec!(30));
        // restano da fatturare solo il conto visione e il DDT di novembre
        let left = s.pending_delivery_notes(None, d(2026, 11, 30), None).unwrap();
        assert_eq!(left.len(), 2);
        assert!(left.iter().any(|x| x.id == vis.id));
    }

    #[test]
    fn stato_di_fatturazione_dei_ddt() {
        let (s, a, b, p, v) = setup();
        let sold = issued(&s, DocKind::DeliveryNote, &a, d(2026, 11, 3), vec![line(&p, dec!(2), &v)], "Vendita");
        let part = issued(&s, DocKind::DeliveryNote, &a, d(2026, 11, 4), vec![line(&p, dec!(4), &v)], "");
        let work = issued(&s, DocKind::DeliveryNote, &b, d(2026, 11, 5), vec![line(&p, dec!(1), &v)], "Conto lavorazione");
        let open = issued(&s, DocKind::DeliveryNote, &b, d(2026, 11, 6), vec![line(&p, dec!(1), &v)], "Vendita");
        let run = s.deferred_invoices(&[sold.id], d(2026, 11, 25), true).unwrap();
        let inv = &run.created[0];
        let mut pinv = s.transform(&[part.id], DocKind::Invoice).unwrap();
        let i = pinv.lines.iter().position(|l| l.kind == LineKind::Item).unwrap();
        pinv.lines[i].qty = dec!(1);
        pinv.date = d(2026, 11, 26);
        let pinv = s.save_document(&pinv).unwrap();
        let bill = s.delivery_billing().unwrap();
        assert_eq!(bill[&sold.id].state, DdtBilling::Invoiced);
        assert_eq!(bill[&sold.id].text(), format!("Fatt. {} del 25/11/2026", inv.number.unwrap()));
        assert_eq!(bill[&sold.id].documents[0].0, inv.id);
        assert_eq!(bill[&part.id].state, DdtBilling::Partial);
        assert_eq!(bill[&part.id].text(), "In parte: Fatt. in bozza del 26/11/2026");
        assert_eq!(bill[&work.id].state, DdtBilling::NotForSale);
        assert_eq!(bill[&work.id].text(), "");
        assert_eq!(bill[&open.id].state, DdtBilling::ToInvoice);
        assert_eq!(bill[&open.id].text(), "Seguirà doc. di vendita");
        // fattura annullata: il DDT torna da fatturare
        s.delete_document(pinv.id).unwrap();
        assert_eq!(s.delivery_billing().unwrap()[&part.id].state, DdtBilling::ToInvoice);
        // la nota di credito sulla fattura non cambia lo stato del DDT
        let mut nc = Document::new(DocKind::CreditNote);
        nc.party_id = Some(a.id);
        nc.date = d(2026, 11, 27);
        s.credit_invoice(&mut nc, inv.id, None).unwrap();
        s.save_document(&nc).unwrap();
        let bill = s.delivery_billing().unwrap();
        assert_eq!(bill[&sold.id].state, DdtBilling::Invoiced);
        assert_eq!(bill[&sold.id].documents.len(), 1);
    }

    #[test]
    fn causali_che_non_si_fatturano() {
        for r in ["Conto lavorazione", "c/lavorazione", "C/VISIONE", "Reso", "Riparazione", "Conto deposito", "Conto approvazione"] {
            assert!(not_for_sale(r), "{}", r);
        }
        for r in ["Vendita", "", "Vendita, trasporto compreso", "Vendita con presa in carico", "Spese di trasporto"] {
            assert!(!not_for_sale(r), "{}", r);
        }
    }

    #[test]
    fn note_interne_modificabili_anche_a_documento_emesso() {
        let (s, a, _, p, v) = setup();
        let ddt = issued(&s, DocKind::DeliveryNote, &a, d(2026, 11, 3), vec![line(&p, dec!(2), &v)], "");
        s.set_internal_notes(ddt.id, "Consegnare al magazzino 2").unwrap();
        let list = s.documents(&DocFilter { kinds: vec![DocKind::DeliveryNote], ..Default::default() }).unwrap();
        assert_eq!(list[0].comment, "Consegnare al magazzino 2");
        let back = s.document(ddt.id).unwrap();
        assert_eq!(back.internal_notes, "Consegnare al magazzino 2");
        assert_eq!(back.status, DocStatus::Issued);
        assert_eq!(back.lines.len(), ddt.lines.len());
        assert!(s.set_internal_notes(999_999, "x").is_err());
    }

    #[test]
    fn fattura_differita_non_anteriore_ai_ddt() {
        let (s, a, _, p, v) = setup();
        let a1 = issued(&s, DocKind::DeliveryNote, &a, d(2026, 10, 20), vec![line(&p, dec!(1), &v)], "");
        let run = s.deferred_invoices(&[a1.id], d(2026, 10, 15), false).unwrap();
        assert!(run.created.is_empty());
        assert_eq!(run.errors.len(), 1);
    }

    #[test]
    fn ddt_fatturato_in_parte_resta_per_il_residuo() {
        let (s, a, _, p, v) = setup();
        let a1 = issued(&s, DocKind::DeliveryNote, &a, d(2026, 10, 2), vec![line(&p, dec!(4), &v)], "");
        let mut inv = s.transform(&[a1.id], DocKind::Invoice).unwrap();
        let i = inv.lines.iter().position(|l| l.kind == LineKind::Item).unwrap();
        inv.lines[i].qty = dec!(1);
        inv.date = d(2026, 10, 5);
        let inv = s.save_document(&inv).unwrap();
        s.issue_document(inv.id).unwrap();
        let pend = s.pending_delivery_notes(None, d(2026, 10, 31), Some(a.id)).unwrap();
        assert_eq!(pend.len(), 1);
        assert!(pend[0].partial);
        assert_eq!(pend[0].amount, dec!(30));
    }

    #[test]
    fn nota_di_credito_cita_la_fattura_e_non_solo_il_ddt() {
        let (s, a, _, p, v) = setup();
        let a1 = issued(&s, DocKind::DeliveryNote, &a, d(2026, 10, 2), vec![line(&p, dec!(3), &v), line(&p, dec!(1), &v)], "");
        let run = s.deferred_invoices(&[a1.id], d(2026, 10, 31), true).unwrap();
        let inv = &run.created[0];
        // dall'elenco o dalla fattura (trasformazione)
        let nc = s.transform(&[inv.id], DocKind::CreditNote).unwrap();
        assert_eq!(nc.related.iter().filter(|r| r.kind == "FATTURA" && r.number == inv.number_label() && r.date == Some(inv.date)).count(), 1);
        assert_eq!(nc.lines[0].kind, LineKind::Text);
        assert_eq!(nc.lines[0].description, format!("Storno fattura n. {} del 31/10/2026", inv.number_label()));
        assert!(nc.causale.starts_with("Storno totale della fattura n."), "{}", nc.causale);
        // nessun DatiDDT nella nota di credito
        assert!(s.ddt_sources(&nc).unwrap().is_empty());

        // da una nota di credito nuova, scegliendo la fattura e una sola riga
        let mut nc2 = Document::new(DocKind::CreditNote);
        let item = inv.lines.iter().position(|l| l.kind == LineKind::Item).unwrap();
        s.credit_invoice(&mut nc2, inv.id, Some(&[item])).unwrap();
        assert_eq!(nc2.party_id, Some(a.id));
        assert_eq!(nc2.related.len(), 1);
        assert_eq!(nc2.lines.len(), 2);
        assert!(nc2.causale.starts_with("Storno parziale"), "{}", nc2.causale);
        // la stessa fattura due volte: un solo riferimento
        s.credit_invoice(&mut nc2, inv.id, Some(&[])).unwrap();
        assert_eq!(nc2.related.len(), 1);
    }

    #[test]
    fn nota_di_credito_rifiuta_fattura_di_altro_cliente_o_in_bozza() {
        let (s, a, b, p, v) = setup();
        let inv = issued(&s, DocKind::Invoice, &a, d(2026, 10, 2), vec![line(&p, dec!(1), &v)], "");
        let mut nc = Document::new(DocKind::CreditNote);
        nc.party_id = Some(b.id);
        assert!(s.credit_invoice(&mut nc, inv.id, None).is_err());
        let mut draft = Document::new(DocKind::Invoice);
        draft.party_id = Some(a.id);
        draft.lines.push(line(&p, dec!(1), &v));
        let draft = s.save_document(&draft).unwrap();
        let mut nc = Document::new(DocKind::CreditNote);
        assert!(s.credit_invoice(&mut nc, draft.id, None).is_err());
    }

    /// Fattura elettronica del documento, come la genera il programma (con i DatiDDT).
    fn fe(s: &Store, doc: &Document) -> openfatt_fatturapa::Generated {
        use openfatt_fatturapa::build::{BuildOptions, DdtRef};
        let company = Company {
            name: "Officina Rossi S.r.l.".into(),
            vat_number: "06363391001".into(),
            tax_code: "06363391001".into(),
            regime_fiscale: "RF01".into(),
            address: "Via Garibaldi".into(),
            civic: "12".into(),
            zip: "38122".into(),
            city: "Trento".into(),
            province: "TN".into(),
            ..Default::default()
        };
        let mut doc = doc.clone();
        doc.party.address = "Via Roma".into();
        doc.party.zip = "38122".into();
        doc.party.province = "TN".into();
        let ddt_refs = s.ddt_sources(&doc).unwrap().into_iter().map(|d| DdtRef { number: d.number, date: Some(d.date), line_indexes: d.line_indexes }).collect();
        let t = calc::compute(&doc).unwrap();
        let g = openfatt_fatturapa::generate(&company, &doc, &t, &BuildOptions { progressivo: "00001".into(), ddt_refs, ..Default::default() }).unwrap();
        let errors: Vec<String> = g.issues.iter().filter(|i| i.severity == openfatt_fatturapa::Severity::Error).map(|i| i.to_string()).collect();
        assert!(errors.is_empty(), "{:#?}\n{}", errors, g.xml);
        g
    }

    #[test]
    fn fattura_elettronica_di_nota_e_saldo() {
        let (s, a, _, p, v) = setup();
        // differita da DDT: TD24 con DatiDDT
        let ddt = issued(&s, DocKind::DeliveryNote, &a, d(2026, 10, 2), vec![line(&p, dec!(3), &v)], "");
        let inv = s.deferred_invoices(&[ddt.id], d(2026, 10, 31), true).unwrap().created.remove(0);
        let g = fe(&s, &inv);
        let dg = &g.fattura.bodies[0].dati_generali;
        assert_eq!(dg.documento.tipo_documento, "TD24");
        assert_eq!(dg.ddt.len(), 1);
        // nota di credito sulla fattura differita: cita la fattura, non i DDT
        let mut nc = s.transform(&[inv.id], DocKind::CreditNote).unwrap();
        nc.date = d(2026, 11, 1);
        let nc = s.save_document(&nc).unwrap();
        let nc = s.issue_document(nc.id).unwrap();
        let g = fe(&s, &nc);
        let dg = &g.fattura.bodies[0].dati_generali;
        assert_eq!(dg.documento.tipo_documento, "TD04");
        assert!(dg.ddt.is_empty());
        assert_eq!(dg.fatture_collegate.len(), 1);
        assert_eq!(dg.fatture_collegate[0].id_documento, inv.number_label());
        assert_eq!(dg.fatture_collegate[0].data, Some(inv.date));
        assert!(g.xml.contains("Storno fattura n."));
        // saldo con acconto detratto: riferimento e prezzo negativo
        let acc = acconto(&s, &a, &v, d(2026, 11, 2), dec!(200));
        assert_eq!(fe(&s, &acc).fattura.bodies[0].dati_generali.documento.tipo_documento, "TD02");
        let mut saldo = Document::new(DocKind::Invoice);
        saldo.date = d(2026, 11, 20);
        saldo.party_id = Some(a.id);
        saldo.lines.push(DocLine { description: "Lavori completati".into(), qty: dec!(1), unit_price: dec!(1000), vat: v.clone(), ..Default::default() });
        s.deduct_down_payments(&mut saldo, &[acc.id]).unwrap();
        let saldo = s.save_document(&saldo).unwrap();
        let saldo = s.issue_document(saldo.id).unwrap();
        let g = fe(&s, &saldo);
        let body = &g.fattura.bodies[0];
        assert_eq!(body.dati_generali.fatture_collegate.len(), 1);
        assert_eq!(body.dati_generali.fatture_collegate[0].id_documento, acc.number_label());
        assert_eq!(body.dati_generali.documento.importo_totale_documento, Some(dec!(976)));
        assert!(g.xml.contains("<PrezzoUnitario>-200.00"), "{}", g.xml);
    }

    fn acconto(s: &Store, a: &Party, v: &VatRef, date: NaiveDate, amount: Decimal) -> Document {
        issued(s, DocKind::DownPaymentInvoice, a, date, vec![DocLine { description: "Acconto lavori".into(), qty: dec!(1), unit_price: amount, vat: v.clone(), ..Default::default() }], "")
    }

    #[test]
    fn fattura_di_saldo_detrae_gli_acconti() {
        let (s, a, _, _, v) = setup();
        let acc1 = acconto(&s, &a, &v, d(2026, 9, 1), dec!(1000));
        let acc2 = acconto(&s, &a, &v, d(2026, 10, 1), dec!(500));
        let open = s.open_down_payments(a.id, 0).unwrap();
        assert_eq!(open.len(), 2);
        assert_eq!(open[0].residual, dec!(1000));
        assert_eq!(open[0].total, dec!(1220));

        let mut saldo = Document::new(DocKind::Invoice);
        saldo.date = d(2026, 11, 2);
        saldo.party_id = Some(a.id);
        saldo.lines.push(DocLine { description: "Impianto completo".into(), qty: dec!(1), unit_price: dec!(5000), vat: v.clone(), ..Default::default() });
        s.deduct_down_payments(&mut saldo, &[acc1.id, acc2.id]).unwrap();
        // riferimenti alle due fatture d'acconto e righe in negativo
        assert_eq!(saldo.related.iter().filter(|r| r.kind == "FATTURA").count(), 2);
        let ded: Vec<&DocLine> = saldo.lines.iter().filter(|l| deducted_down_payment(l).is_some()).collect();
        assert_eq!(ded.len(), 2);
        assert_eq!(ded[0].unit_price, dec!(-1000));
        assert_eq!(ded[0].description, format!("Acconto fattura n. {} del 01/09/2026", acc1.number_label()));
        assert!(s.deduct_down_payments(&mut saldo, &[acc1.id]).is_err());
        let saldo = s.save_document(&saldo).unwrap();
        let t = calc::compute(&saldo).unwrap();
        assert_eq!(t.taxable, dec!(3500));
        assert_eq!(t.total, dec!(4270));
        let saldo = s.issue_document(saldo.id).unwrap();
        // acconti detratti: non si propongono più
        assert!(s.open_down_payments(a.id, 0).unwrap().is_empty());
        // nella fattura aperta le sue detrazioni non contano
        assert_eq!(s.open_down_payments(a.id, saldo.id).unwrap().len(), 2);
        // statistiche: il saldo conta per l'intero lavoro
        let st = s.statistics(crate::StatBy::Customer, d(2026, 1, 1), d(2026, 12, 31)).unwrap();
        assert_eq!(st.iter().map(|r| r.revenue).sum::<Decimal>(), dec!(5000));
        // annullando il saldo gli acconti tornano da detrarre
        s.cancel_document(saldo.id).unwrap();
        assert_eq!(s.open_down_payments(a.id, 0).unwrap().len(), 2);
    }

    #[test]
    fn saldo_dall_elenco_degli_acconti() {
        let (s, a, _, _, v) = setup();
        let acc = acconto(&s, &a, &v, d(2026, 9, 1), dec!(300));
        let inv = s.transform(&[acc.id], DocKind::Invoice).unwrap();
        assert_eq!(inv.party_id, Some(a.id));
        assert_eq!(inv.lines.len(), 1);
        assert_eq!(inv.lines[0].unit_price, dec!(-300));
        assert_eq!(inv.related[0].number, acc.number_label());
    }
}
