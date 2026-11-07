//! Raccolta dei dati per registri IVA e liquidazioni (il calcolo è in openfatt_core::vat).

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use openfatt_core::calc;
use openfatt_core::model::{DocKind, DocStatus, Document};
use openfatt_core::vat::{DeferredRow, Liquidation, Period, PeriodInputs, PeriodTotals, Periodicity, RateAmount, Register, RegisterRow, RetailRow};

use crate::{date_from_sql, dec_from_sql, DocFilter, Result, Store};

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
    fn purchase_date(&self, d: &Document) -> NaiveDate {
        let received: Option<String> = self
            .conn()
            .query_row("SELECT created_at FROM einvoices WHERE document_id = ?1 AND direction = 'IN' ORDER BY id LIMIT 1", [d.id], |r| r.get(0))
            .ok();
        received.map(|s| date_from_sql(&s)).filter(|r| *r >= d.date).unwrap_or(d.date)
    }

    /// Registro IVA del periodo (documenti emessi o registrati). Nel registro acquisti
    /// compaiono anche le integrazioni in reverse charge, segnate e fuori dai totali.
    pub fn vat_register(&self, register: Register, from: NaiveDate, to: NaiveDate) -> Result<Vec<RegisterRow>> {
        let mut out = Vec::new();
        match register {
            Register::Sales => {
                let list = self.documents(&DocFilter { kinds: SALES.to_vec(), status: Some(DocStatus::Issued), from: Some(from), to: Some(to), ..Default::default() })?;
                for s in list {
                    let d = self.document(s.id)?;
                    if d.kind == DocKind::SelfInvoice && SELF_OUTSIDE.contains(&fe_type(&d).as_str()) {
                        continue;
                    }
                    if let Some(r) = self.row_from(&d, Register::Sales) {
                        out.push(r);
                    }
                }
            }
            Register::Purchases => {
                // registrazione fino a qualche settimana dopo la data del documento
                let list = self.documents(&DocFilter { kinds: PURCHASES.to_vec(), status: Some(DocStatus::Issued), from: Some(from - chrono::Duration::days(400)), to: Some(to), ..Default::default() })?;
                for s in list {
                    let d = self.document(s.id)?;
                    let reg = self.purchase_date(&d);
                    if reg < from || reg > to {
                        continue;
                    }
                    if let Some(mut r) = self.row_from(&d, Register::Purchases) {
                        r.date = reg;
                        out.push(r);
                    }
                }
                for mut r in self.vat_register(Register::Sales, from, to)?.into_iter().filter(|r| r.reverse_charge) {
                    r.kind_label = format!("{} (integrazione)", r.kind_label);
                    out.push(r);
                }
            }
        }
        out.sort_by(|a, b| a.date.cmp(&b.date).then(a.number.cmp(&b.number)));
        Ok(out)
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
        let list = self.documents(&DocFilter { kinds, status: Some(DocStatus::Issued), from: Some(from - chrono::Duration::days(370)), to: Some(to), ..Default::default() })?;
        for s in list {
            let d = self.document(s.id)?;
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
