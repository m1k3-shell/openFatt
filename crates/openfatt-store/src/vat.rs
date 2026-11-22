//! Raccolta dei dati per registri IVA e liquidazioni (il calcolo è in openfatt_core::vat).

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use openfatt_core::calc;
use openfatt_core::model::{DocKind, DocStatus, Document};
use openfatt_core::vat::{DeferredRow, Liquidation, Period, PeriodInputs, PeriodTotals, Periodicity, RateAmount, Register, RegisterRow, RetailRow};

use rusqlite::params;

use crate::{date_from_sql, date_to_sql, dec_from_sql, Result, Store};

/// Impostazioni IVA dell'azienda.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VatSettings {
    pub periodicity: Periodicity,
    /// Dichiarante della Comunicazione (per le società: es. il legale rappresentante).
    pub declarant_cf: String,
    pub declarant_role: u8,
    /// Credito IVA dell'anno precedente risultante dalla dichiarazione annuale (promemoria).
    pub prev_year_credit: Decimal,
}

const SALES: [DocKind; 7] = [DocKind::Invoice, DocKind::AccompanyingInvoice, DocKind::CreditNote, DocKind::DebitNote, DocKind::DownPaymentInvoice, DocKind::Fee, DocKind::SelfInvoice];
const PURCHASES: [DocKind; 2] = [DocKind::PurchaseInvoice, DocKind::PurchaseCreditNote];

/// Autofatture TD20-TD23 e TD29: l'imposta si versa direttamente, fuori dalla liquidazione.
const SELF_OUTSIDE: [&str; 5] = ["TD20", "TD21", "TD22", "TD23", "TD29"];
/// Integrazioni e autofatture in reverse charge: imposta a debito e detratta.
const SELF_REVERSE: [&str; 5] = ["TD16", "TD17", "TD18", "TD19", "TD28"];

fn fe_type(d: &Document) -> String {
    if d.fe_type.trim().is_empty() {
        d.kind.default_fe_type().unwrap_or("").to_string()
    } else {
        d.fe_type.trim().to_string()
    }
}

fn period_key(p: Period) -> String {
    format!("{}-{}{:02}", p.year, if p.quarterly { "T" } else { "M" }, p.index)
}

impl Store {
    pub fn vat_settings(&self) -> Result<VatSettings> {
        let mut s: VatSettings = self.get_setting("vat")?;
        if s.declarant_role == 0 {
            s.declarant_role = 1;
        }
        Ok(s)
    }

    pub fn set_vat_settings(&self, s: &VatSettings) -> Result<()> {
        self.set_setting("vat", s)
    }

    pub fn vat_inputs(&self, p: Period) -> Result<PeriodInputs> {
        let all: BTreeMap<String, PeriodInputs> = self.get_setting("vat_inputs")?;
        Ok(all.get(&period_key(p)).cloned().unwrap_or_default())
    }

    pub fn set_vat_inputs(&self, p: Period, i: &PeriodInputs) -> Result<()> {
        let mut all: BTreeMap<String, PeriodInputs> = self.get_setting("vat_inputs")?;
        all.insert(period_key(p), i.clone());
        self.set_setting("vat_inputs", &all)
    }

    fn row_from(&self, d: &Document, register: Register) -> Option<RegisterRow> {
        let t = calc::compute(d).ok()?;
        let sign = if d.kind.is_credit() { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
        let td = fe_type(d);
        let rates = t
            .vat_rows
            .iter()
            .map(|r| RateAmount { rate: r.rate, natura: r.natura.clone(), taxable: d.to_eur(r.taxable) * sign, tax: d.to_eur(r.tax) * sign, liability: if r.liability.is_empty() { "I".into() } else { r.liability.clone() } })
            .collect();
        let party = d.party.display_name();
        let vat = if d.party.vat_number.trim().is_empty() { d.party.tax_code.trim().to_string() } else { format!("{}{}", d.party.vat_country.trim(), d.party.vat_number.trim()) };
        Some(RegisterRow {
            document_id: d.id,
            date: d.date,
            doc_date: d.date,
            number: d.number_label(),
            external_number: d.external_number.clone(),
            kind_label: d.kind.label().to_string(),
            fe_type: td.clone(),
            party,
            party_vat: vat,
            rates,
            total: d.to_eur(t.total) * sign,
            reverse_charge: register == Register::Sales && d.kind == DocKind::SelfInvoice && SELF_REVERSE.contains(&td.as_str()),
        })
    }

    /// Data di registrazione di un acquisto: quella di ricezione della fattura elettronica,
    /// se collegata, altrimenti la data del documento.
    fn purchase_date(&self, id: i64, date: NaiveDate) -> NaiveDate {
        let received: Option<String> = self
            .conn()
            .query_row("SELECT created_at FROM einvoices WHERE document_id = ?1 AND direction = 'IN' ORDER BY id LIMIT 1", [id], |r| r.get(0))
            .ok();
        received.map(|s| date_from_sql(&s)).filter(|r| *r >= date).unwrap_or(date)
    }

    /// Registro IVA del periodo (documenti emessi o registrati). Nel registro acquisti
    /// compaiono anche le integrazioni in reverse charge, segnate e fuori dai totali.
    pub fn vat_register(&self, register: Register, from: NaiveDate, to: NaiveDate) -> Result<Vec<RegisterRow>> {
        let mut out = Vec::new();
        match register {
            Register::Sales => out = self.sales_rows(&SALES, from, to)?,
            Register::Purchases => {
                // registrazione fino a qualche settimana dopo la data del documento
                for (id, key) in self.issued_keys(&PURCHASES, from - chrono::Duration::days(400), to)? {
                    let Some(mut r) = self.cached_row(id, key, true, |d| self.row_from(d, Register::Purchases))? else { continue };
                    r.date = self.purchase_date(id, r.doc_date);
                    if r.date >= from && r.date <= to {
                        out.push(r);
                    }
                }
                // le integrazioni in reverse charge sono solo autofatture: non serve rileggere tutte le vendite
                for mut r in self.sales_rows(&[DocKind::SelfInvoice], from, to)?.into_iter().filter(|r| r.reverse_charge) {
                    r.kind_label = format!("{} (integrazione)", r.kind_label);
                    out.push(r);
                }
            }
        }
        out.sort_by(|a, b| a.date.cmp(&b.date).then(a.number.cmp(&b.number)));
        Ok(out)
    }

    /// Righe del registro vendite per i tipi di documento indicati.
    fn sales_rows(&self, kinds: &[DocKind], from: NaiveDate, to: NaiveDate) -> Result<Vec<RegisterRow>> {
        let mut out = Vec::new();
        for (id, key) in self.issued_keys(kinds, from, to)? {
            let row = self.cached_row(id, key, false, |d| {
                if d.kind == DocKind::SelfInvoice && SELF_OUTSIDE.contains(&fe_type(d).as_str()) {
                    None
                } else {
                    self.row_from(d, Register::Sales)
                }
            })?;
            out.extend(row);
        }
        Ok(out)
    }

    /// Documenti emessi del periodo con una chiave che cambia a ogni loro modifica.
    pub(crate) fn issued_keys(&self, kinds: &[DocKind], from: NaiveDate, to: NaiveDate) -> Result<Vec<(i64, u64)>> {
        let marks: Vec<String> = kinds.iter().map(|k| format!("'{}'", k.code())).collect();
        let mut st = self.conn().prepare_cached(&format!(
            "SELECT id, version, s_rev, kind, date, total, created_at FROM documents WHERE status = ?1 AND kind IN ({}) AND date >= ?2 AND date <= ?3
             ORDER BY date, number, id",
            marks.join(",")
        ))?;
        let rows = st.query_map(params![DocStatus::Issued.code(), date_to_sql(from), date_to_sql(to)], |r| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            r.get::<_, i64>(1)?.hash(&mut h);
            r.get::<_, i64>(2)?.hash(&mut h);
            r.get::<_, String>(3)?.hash(&mut h);
            r.get::<_, String>(4)?.hash(&mut h);
            r.get::<_, Option<String>>(5)?.hash(&mut h);
            r.get::<_, Option<String>>(6)?.hash(&mut h);
            Ok((r.get::<_, i64>(0)?, h.finish()))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Riga del registro calcolata una volta per versione del documento.
    fn cached_row(&self, id: i64, key: u64, purchase: bool, make: impl FnOnce(&Document) -> Option<RegisterRow>) -> Result<Option<RegisterRow>> {
        if let Some((k, row)) = self.vat_rows.borrow().get(&(id, purchase)) {
            if *k == key {
                return Ok(row.clone());
            }
        }
        let d = self.document(id)?;
        let row = make(&d);
        self.vat_rows.borrow_mut().insert((id, purchase), (key, row.clone()));
        Ok(row)
    }

    /// Registro dei corrispettivi: vendite al banco per giorno e aliquota.
    pub fn retail_register(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<RetailRow>> {
        Ok(self
            .retail_summary(from, to)?
            .into_iter()
            .filter_map(|d| {
                Some(RetailRow { date: d.date?, count: d.count, gross: d.gross, rates: d.by_rate.into_iter().map(|(l, taxable, tax, _)| (l, taxable, tax)).collect() })
            })
            .collect())
    }

    /// Imposta ad esigibilità differita che diventa esigibile (vendite) o detraibile
    /// (acquisti) nel periodo: in proporzione agli incassi/pagamenti, oppure per intero
    /// (il residuo) quando è trascorso un anno dall'operazione.
    pub fn deferred_vat(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<DeferredRow>> {
        let mut out = Vec::new();
        let kinds: Vec<DocKind> = SALES.iter().chain(PURCHASES.iter()).copied().collect();
        // solo i documenti a esigibilità differita (colonna di riepilogo): prima si
        // rileggevano per intero tutti i documenti degli ultimi tredici mesi
        let marks: Vec<String> = kinds.iter().map(|k| format!("'{}'", k.code())).collect();
        let ids: Vec<i64> = {
            let mut st = self.conn().prepare_cached(&format!(
                "SELECT id FROM documents WHERE status = ?1 AND s_liability = 'D' AND kind IN ({}) AND date >= ?2 AND date <= ?3 ORDER BY date, id",
                marks.join(",")
            ))?;
            let rows = st.query_map(params![DocStatus::Issued.code(), date_to_sql(from - chrono::Duration::days(370)), date_to_sql(to)], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for id in ids {
            let d = self.document(id)?;
            let Ok(t) = calc::compute(&d) else { continue };
            let tax_d: Decimal = d.to_eur(t.vat_rows.iter().filter(|r| r.liability == "D").map(|r| r.tax).sum());
            if tax_d.is_zero() || t.total.is_zero() {
                continue;
            }
            let register = if d.kind.is_purchase() { Register::Purchases } else { Register::Sales };
            let sign = if d.kind.is_credit() { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
            let expiry = d.date + chrono::Duration::days(365);
            let mut st = self.conn().prepare("SELECT date, amount FROM cash_entries WHERE document_id = ?1")?;
            let moves: Vec<(NaiveDate, Decimal)> = st.query_map([d.id], |r| Ok((date_from_sql(&r.get::<_, String>(0)?), dec_from_sql(Some(r.get::<_, String>(1)?)))))?.collect::<std::result::Result<_, _>>()?;
            let share = |a: Decimal| (a.abs() / t.total.abs()).min(Decimal::ONE) * tax_d;
            let before: Decimal = moves.iter().filter(|(dt, _)| *dt < from && *dt < expiry).map(|(_, a)| share(*a)).sum();
            let within: Decimal = moves.iter().filter(|(dt, _)| *dt >= from && *dt <= to && *dt < expiry).map(|(_, a)| share(*a)).sum();
            let mut tax = within;
            if expiry >= from && expiry <= to {
                tax = (tax_d - before).max(Decimal::ZERO);
            }
            let tax = openfatt_core::money::round2(tax.min(tax_d - before).max(Decimal::ZERO)) * sign;
            if !tax.is_zero() {
                out.push(DeferredRow { document_id: d.id, label: format!("{} {} del {} - {}", d.kind.label(), d.number_label(), d.date.format("%d/%m/%Y"), d.party.display_name()), register: Some(register), tax });
            }
        }
        Ok(out)
    }

    /// Totali dei righi VP2-VP5 per il periodo.
    pub fn vat_period_totals(&self, p: Period) -> Result<PeriodTotals> {
        let (from, to) = (p.first(), p.last());
        let sales = self.vat_register(Register::Sales, from, to)?;
        let purchases = self.vat_register(Register::Purchases, from, to)?;
        let retail = self.retail_register(from, to)?;
        let deferred = self.deferred_vat(from, to)?;
        Ok(PeriodTotals::compute(&sales, &purchases, &retail, &deferred))
    }

    /// Liquidazioni dell'anno fino al periodo indicato (compreso), con i riporti.
    pub fn vat_liquidations(&self, year: i32, upto: u32) -> Result<Vec<(Period, Liquidation)>> {
        let s = self.vat_settings()?;
        let mut out: Vec<(Period, Liquidation)> = Vec::new();
        for i in 1..=upto {
            let p = Period::new(year, i, s.periodicity);
            let l = openfatt_core::vat::liquidate(p, self.vat_period_totals(p)?, self.vat_inputs(p)?, out.last().map(|x| &x.1));
            out.push((p, l));
        }
        Ok(out)
    }
}
