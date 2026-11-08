//! Motore di calcolo dei documenti.
//!
//! Regole (dalle specifiche FatturaPA e dall'Elenco controlli SdI v2.0):
//! - sconti percentuali in cascata sul prezzo unitario (controllo 00423);
//! - PrezzoTotale = prezzo unitario scontato x quantità, arrotondato al centesimo
//!   (tolleranza SdI 0,01);
//! - riepilogo per aliquota/natura: imponibile = somma dei PrezzoTotale + contributi cassa
//!   (controllo 00422), imposta = imponibile x aliquota / 100 arrotondata half-up (00421);
//! - ritenute calcolate sulle righe marcate (e sul contributo cassa se soggetto);
//! - bollo virtuale automatico oltre 77,47 euro di operazioni senza IVA soggette a bollo.
//!
//! Lo stesso risultato alimenta schermo, PDF e XML: non ci sono calcoli paralleli.

use std::collections::BTreeMap;

use chrono::{Datelike, Duration, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::model::*;
use crate::money::{parse_dec, pct_of, round2, round_dp};

/// Soglia oltre la quale si applica l'imposta di bollo (art. 13 DPR 642/72).
pub fn stamp_threshold() -> Decimal {
    Decimal::new(7747, 2)
}

/// Nature soggette a imposta di bollo quando l'importo supera la soglia.
/// Escluse: esportazioni e cessioni intra/San Marino (N3.1-N3.4), reverse charge (N6.x),
/// regime del margine (N5) e IVA assolta in altro Stato (N7).
pub const STAMP_NATURES: &[&str] = &["N1", "N2.1", "N2.2", "N3.5", "N3.6", "N4"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscountError(pub String);

/// Interpreta "10+5+2,5" come lista di percentuali di sconto.
/// Valori negativi rappresentano maggiorazioni.
pub fn parse_discounts(s: &str) -> Result<Vec<Decimal>, DiscountError> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for part in t.split('+') {
        let p = part.trim().trim_end_matches('%').trim();
        if p.is_empty() {
            continue;
        }
        match parse_dec(p) {
            Some(v) if v <= Decimal::ONE_HUNDRED => out.push(v),
            _ => return Err(DiscountError(format!("Sconto non valido: '{}'", part.trim()))),
        }
    }
    Ok(out)
}

/// Applica gli sconti in cascata a un prezzo unitario, senza arrotondare.
pub fn apply_cascade(price: Decimal, discounts: &[Decimal]) -> Decimal {
    let mut p = price;
    for d in discounts {
        p -= pct_of(p, *d);
    }
    p
}

/// Sconto composto equivalente: 10+5 = 14,5%.
pub fn equivalent_discount(discounts: &[Decimal]) -> Decimal {
    Decimal::ONE_HUNDRED - apply_cascade(Decimal::ONE_HUNDRED, discounts)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LineCalc {
    /// Indice nella lista righe del documento; None per righe generate (bollo).
    pub index: Option<usize>,
    pub description: String,
    pub qty: Decimal,
    /// Prezzo unitario netto IVA, prima degli sconti (8 decimali).
    pub unit_net: Decimal,
    /// Sconti percentuali applicati (riga + documento).
    pub discounts: Vec<Decimal>,
    /// Prezzo unitario netto dopo gli sconti, non arrotondato.
    pub unit_discounted: Decimal,
    /// PrezzoTotale arrotondato al centesimo.
    pub total: Decimal,
    pub vat: VatRef,
    pub withholding: bool,
    pub pension_fund: bool,
    pub is_text: bool,
    pub is_generated: bool,
    /// Righe di subtotale: somma delle righe dall'ultimo subtotale
    #[serde(default)]
    pub subtotal: Option<Decimal>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VatRow {
    pub rate: Decimal,
    pub natura: Option<String>,
    pub legal_ref: String,
    /// Somma dei PrezzoTotale delle righe con questa aliquota.
    pub lines_total: Decimal,
    /// Contributo cassa su questa aliquota.
    pub pension_fund: Decimal,
    /// Arrotondamento (solo documenti a prezzi ivati).
    pub rounding: Decimal,
    pub taxable: Decimal,
    pub tax: Decimal,
    pub liability: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PensionFundCalc {
    pub kind: String,
    pub rate: Decimal,
    pub base: Decimal,
    pub amount: Decimal,
    pub vat: VatRef,
    pub subject_to_withholding: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WithholdingCalc {
    pub kind: String,
    pub rate: Decimal,
    pub base: Decimal,
    pub amount: Decimal,
    pub payment_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Totals {
    pub lines: Vec<LineCalc>,
    pub vat_rows: Vec<VatRow>,
    pub taxable: Decimal,
    pub tax: Decimal,
    /// Totale documento (imponibile + IVA), come ImportoTotaleDocumento.
    pub total: Decimal,
    pub pension_fund: Option<PensionFundCalc>,
    pub withholdings: Vec<WithholdingCalc>,
    pub withholding_total: Decimal,
    /// Importo del bollo, se dovuto.
    pub stamp: Option<Decimal>,
    pub stamp_charged: bool,
    /// Netto a pagare (totale - ritenute - IVA in scissione).
    pub net_to_pay: Decimal,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalcError(pub String);

impl std::fmt::Display for CalcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CalcError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct VatKey {
    rate: Decimal,
    natura: String,
}

/// Aliquota da usare per le righe descrittive: la prima aliquota presente fra le righe con importo.
fn text_line_vat(doc: &Document) -> VatRef {
    doc.lines
        .iter()
        .find(|l| l.kind == LineKind::Item)
        .map(|l| l.vat.clone())
        .unwrap_or(VatRef { vat_id: None, rate: Decimal::from(22), natura: None, legal_ref: String::new(), split: false })
}

/// Calcola tutti i totali di un documento.
pub fn compute(doc: &Document) -> Result<Totals, CalcError> {
    let doc_discounts = parse_discounts(&doc.discount).map_err(|e| CalcError(e.0))?;
    let mut warnings = Vec::new();
    let mut lines = Vec::new();
    let text_vat = text_line_vat(doc);

    // Somme a prezzi ivati per aliquota (solo prezzi IVA inclusa)
    let mut gross_by_key: BTreeMap<VatKey, Decimal> = BTreeMap::new();

    // somma delle righe dall'ultimo subtotale
    let mut since_subtotal = Decimal::ZERO;
    for (i, l) in doc.lines.iter().enumerate() {
        if l.kind == LineKind::Text || l.kind == LineKind::Subtotal {
            let subtotal = if l.kind == LineKind::Subtotal {
                let v = since_subtotal;
                since_subtotal = Decimal::ZERO;
                Some(v)
            } else {
                None
            };
            let description = if l.kind == LineKind::Subtotal && l.description.trim().is_empty() { "Subtotale".to_string() } else { l.description.clone() };
            lines.push(LineCalc {
                index: Some(i),
                description,
                qty: Decimal::ZERO,
                unit_net: Decimal::ZERO,
                discounts: vec![],
                unit_discounted: Decimal::ZERO,
                total: Decimal::ZERO,
                vat: text_vat.clone(),
                withholding: false,
                pension_fund: false,
                is_text: true,
                is_generated: false,
                subtotal,
            });
            continue;
        }
        // documenti interni di magazzino: nessuna imposta, nessun controllo dell'IVA
        if l.vat.rate.is_zero() && l.vat.natura.as_deref().unwrap_or("").is_empty() && !doc.kind.is_internal() {
            return Err(CalcError(format!(
                "Riga {}: aliquota 0% senza natura dell'operazione (obbligatoria, errore SdI 00400)",
                i + 1
            )));
        }
        if !l.vat.rate.is_zero() && l.vat.natura.as_deref().map(|n| !n.is_empty()).unwrap_or(false) {
            return Err(CalcError(format!(
                "Riga {}: natura indicata con aliquota diversa da zero (errore SdI 00401)",
                i + 1
            )));
        }
        // riga calcolata: percentuale della somma delle righe precedenti
        if let Some(pct) = l.calc_percent {
            let base: Decimal = lines.iter().filter(|x| !x.is_text).map(|x| x.total).sum();
            let amount = round2(pct_of(base, pct));
            if doc.prices_include_vat {
                let key = VatKey { rate: l.vat.rate, natura: l.vat.natura.clone().unwrap_or_default() };
                *gross_by_key.entry(key).or_default() += round2(amount + pct_of(amount, l.vat.rate));
            }
            lines.push(LineCalc {
                index: Some(i),
                description: l.description.clone(),
                qty: Decimal::ONE,
                unit_net: amount,
                discounts: vec![],
                unit_discounted: amount,
                total: amount,
                vat: l.vat.clone(),
                withholding: l.withholding,
                pension_fund: l.pension_fund,
                is_text: false,
                is_generated: false,
                subtotal: None,
            });
            since_subtotal += amount;
            continue;
        }
        let mut discounts = parse_discounts(&l.discounts).map_err(|e| CalcError(format!("Riga {}: {}", i + 1, e.0)))?;
        discounts.extend(doc_discounts.iter().copied());

        let (unit_net, unit_discounted, total) = if doc.prices_include_vat {
            let divisor = Decimal::ONE + l.vat.rate / Decimal::ONE_HUNDRED;
            let gross_disc = apply_cascade(l.unit_price, &discounts);
            let gross_total = round2(gross_disc * l.qty);
            let key = VatKey { rate: l.vat.rate, natura: l.vat.natura.clone().unwrap_or_default() };
            *gross_by_key.entry(key).or_default() += gross_total;
            let unit_net = round_dp(l.unit_price / divisor, 8);
            let unit_disc = apply_cascade(unit_net, &discounts);
            (unit_net, unit_disc, round2(unit_disc * l.qty))
        } else {
            let unit_disc = apply_cascade(l.unit_price, &discounts);
            (l.unit_price, unit_disc, round2(unit_disc * l.qty))
        };

        lines.push(LineCalc {
            index: Some(i),
            description: l.description.clone(),
            qty: l.qty,
            unit_net,
            discounts,
            unit_discounted,
            total,
            vat: l.vat.clone(),
            withholding: l.withholding,
            pension_fund: l.pension_fund,
            is_text: false,
            is_generated: false,
            subtotal: None,
        });
        since_subtotal += total;

        // contributo ambientale (RAEE, pile...): riga generata con la stessa quantità e IVA
        if !l.eco_fee.is_zero() {
            let eco_total = round2(l.eco_fee * l.qty);
            if doc.prices_include_vat {
                let key = VatKey { rate: l.vat.rate, natura: l.vat.natura.clone().unwrap_or_default() };
                *gross_by_key.entry(key).or_default() += round2(eco_total + pct_of(eco_total, l.vat.rate));
            }
            lines.push(LineCalc {
                index: None,
                description: if l.eco_label.trim().is_empty() { "Contributo ambientale".into() } else { l.eco_label.trim().to_string() },
                qty: l.qty,
                unit_net: l.eco_fee,
                discounts: vec![],
                unit_discounted: l.eco_fee,
                total: eco_total,
                vat: l.vat.clone(),
                withholding: false,
                pension_fund: false,
                is_text: false,
                is_generated: true,
                subtotal: None,
            });
            since_subtotal += eco_total;
        }
    }

    // Contributo cassa previdenziale
    let pension = doc.pension_fund.as_ref().map(|pf| {
        let base: Decimal = lines.iter().filter(|l| !l.is_text && l.pension_fund).map(|l| l.total).sum();
        PensionFundCalc {
            kind: pf.kind.clone(),
            rate: pf.rate,
            base,
            amount: round2(pct_of(base, pf.rate)),
            vat: pf.vat.clone(),
            subject_to_withholding: pf.subject_to_withholding,
        }
    });
    if let Some(pf) = &pension {
        if pf.base.is_zero() {
            warnings.push("Cassa previdenziale indicata ma nessuna riga è soggetta a contributo".into());
        }
    }

    // Riepilogo IVA provvisorio, prima del bollo
    let build_rows = |lines: &[LineCalc]| -> BTreeMap<VatKey, VatRow> {
        let mut rows: BTreeMap<VatKey, VatRow> = BTreeMap::new();
        for l in lines.iter() {
            let key = VatKey { rate: l.vat.rate, natura: l.vat.natura.clone().unwrap_or_default() };
            let row = rows.entry(key).or_insert_with(|| VatRow {
                rate: l.vat.rate,
                natura: l.vat.natura.clone().filter(|n| !n.is_empty()),
                legal_ref: l.vat.legal_ref.clone(),
                lines_total: Decimal::ZERO,
                pension_fund: Decimal::ZERO,
                rounding: Decimal::ZERO,
                taxable: Decimal::ZERO,
                tax: Decimal::ZERO,
                liability: String::new(),
            });
            row.lines_total += l.total;
            if row.legal_ref.is_empty() && !l.vat.legal_ref.is_empty() {
                row.legal_ref = l.vat.legal_ref.clone();
            }
        }
        rows
    };

    let mut rows = build_rows(&lines);

    // Bollo
    let stamp_base: Decimal = rows
        .values()
        .filter(|r| r.rate.is_zero() && r.natura.as_deref().map(|n| STAMP_NATURES.contains(&n)).unwrap_or(false))
        .map(|r| r.lines_total)
        .sum();
    let stamp_due = match doc.stamp.mode {
        StampMode::Yes => true,
        StampMode::No => false,
        StampMode::Auto => stamp_base > stamp_threshold(),
    };
    let stamp = if stamp_due && doc.kind.is_sale() { Some(doc.stamp.amount) } else { None };
    let stamp_charged = stamp.is_some() && doc.stamp.charge_to_customer;
    if stamp_charged {
        let vat = VatRef {
            vat_id: None,
            rate: Decimal::ZERO,
            natura: Some("N1".into()),
            legal_ref: "Escluso art. 15 DPR 633/72".into(),
            split: false,
        };
        lines.push(LineCalc {
            index: None,
            description: "Rimborso imposta di bollo".into(),
            qty: Decimal::ONE,
            unit_net: doc.stamp.amount,
            discounts: vec![],
            unit_discounted: doc.stamp.amount,
            total: doc.stamp.amount,
            vat,
            withholding: false,
            pension_fund: false,
            is_text: false,
            is_generated: true,
            subtotal: None,
        });
        rows = build_rows(&lines);
    }

    // Contributo cassa nel riepilogo
    if let Some(pf) = &pension {
        if !pf.amount.is_zero() {
            let key = VatKey { rate: pf.vat.rate, natura: pf.vat.natura.clone().unwrap_or_default() };
            let row = rows.entry(key).or_insert_with(|| VatRow {
                rate: pf.vat.rate,
                natura: pf.vat.natura.clone().filter(|n| !n.is_empty()),
                legal_ref: pf.vat.legal_ref.clone(),
                lines_total: Decimal::ZERO,
                pension_fund: Decimal::ZERO,
                rounding: Decimal::ZERO,
                taxable: Decimal::ZERO,
                tax: Decimal::ZERO,
                liability: String::new(),
            });
            row.pension_fund += pf.amount;
        }
    }

    // un'aliquota "sp" (scissione dei pagamenti) vale per tutto il documento
    let liability = if doc.lines.iter().any(|l| l.vat.split) {
        "S".to_string()
    } else if doc.vat_liability.trim().is_empty() {
        "I".to_string()
    } else {
        doc.vat_liability.trim().to_string()
    };

    let mut vat_rows = Vec::new();
    for (key, mut row) in rows {
        let computed = row.lines_total + row.pension_fund;
        if doc.prices_include_vat && !row.rate.is_zero() {
            if let Some(gross) = gross_by_key.get(&key) {
                // imponibile scorporato dal totale ivato; l'eventuale differenza con la
                // somma delle righe va in Arrotondamento (tolleranza SdI 00422: 1 euro)
                let gross = *gross + row.pension_fund * (Decimal::ONE + row.rate / Decimal::ONE_HUNDRED);
                let taxable = round2(gross * Decimal::ONE_HUNDRED / (Decimal::ONE_HUNDRED + row.rate));
                row.rounding = taxable - computed;
                row.taxable = taxable;
            } else {
                row.taxable = computed;
            }
        } else {
            row.taxable = computed;
        }
        row.tax = round2(pct_of(row.taxable, row.rate));
        row.liability = if row.rate.is_zero() {
            String::new()
        } else if row.natura.as_deref().map(|n| n.starts_with("N6")).unwrap_or(false) {
            "I".into()
        } else {
            liability.clone()
        };
        vat_rows.push(row);
    }
    // ordine: aliquote decrescenti, poi nature
    vat_rows.sort_by(|a, b| b.rate.cmp(&a.rate).then(a.natura.cmp(&b.natura)));

    if liability == "S" && vat_rows.iter().any(|r| r.natura.as_deref().map(|n| n.starts_with("N6")).unwrap_or(false)) {
        return Err(CalcError(
            "Scissione dei pagamenti non compatibile con inversione contabile N6 (errore SdI 00420)".into(),
        ));
    }

    let taxable: Decimal = vat_rows.iter().map(|r| r.taxable).sum();
    let tax: Decimal = vat_rows.iter().map(|r| r.tax).sum();
    let total = taxable + tax;

    // Ritenute
    let mut withholdings = Vec::new();
    for w in &doc.withholdings {
        let mut base: Decimal = lines.iter().filter(|l| !l.is_text && l.withholding).map(|l| l.total).sum();
        if let Some(pf) = &pension {
            if pf.subject_to_withholding {
                base += pf.amount;
            }
        }
        let base = round2(pct_of(base, w.base_percent));
        if base.is_zero() {
            warnings.push(format!("Ritenuta {} indicata ma nessuna riga è soggetta a ritenuta", w.kind));
        }
        withholdings.push(WithholdingCalc {
            kind: w.kind.clone(),
            rate: w.rate,
            base,
            amount: round2(pct_of(base, w.rate)),
            payment_reason: w.payment_reason.clone(),
        });
    }
    let withholding_total: Decimal = withholdings.iter().map(|w| w.amount).sum();

    let split_tax: Decimal = if liability == "S" {
        vat_rows.iter().filter(|r| r.liability == "S").map(|r| r.tax).sum()
    } else {
        Decimal::ZERO
    };
    let net_to_pay = total - withholding_total - split_tax;

    if lines.iter().all(|l| l.is_text) {
        warnings.push("Il documento non contiene righe con importo".into());
    }

    Ok(Totals {
        lines,
        vat_rows,
        taxable,
        tax,
        total,
        pension_fund: pension,
        withholdings,
        withholding_total,
        stamp,
        stamp_charged,
        net_to_pay,
        warnings,
    })
}

/// Calcolo inverso di una riga: prezzo unitario che dà il totale indicato.
///
/// `target` è il totale della riga, IVA inclusa se `target_with_vat`; il prezzo restituito
/// è espresso come i prezzi del documento (ivato se `prices_include_vat`), a 6 decimali.
pub fn unit_price_for_total(target: Decimal, qty: Decimal, discounts: &str, vat_rate: Decimal, target_with_vat: bool, prices_include_vat: bool) -> Option<Decimal> {
    if qty.is_zero() {
        return None;
    }
    let disc = parse_discounts(discounts).ok()?;
    let factor = apply_cascade(Decimal::ONE, &disc);
    if factor.is_zero() {
        return None;
    }
    let mult = Decimal::ONE + vat_rate / Decimal::ONE_HUNDRED;
    // totale riga espresso come i prezzi del documento
    let line_total = match (target_with_vat, prices_include_vat) {
        (true, false) => target / mult,
        (false, true) => target * mult,
        _ => target,
    };
    let price = round_dp(line_total / (qty * factor), 6);
    // piccola correzione per ritrovare esattamente il totale al centesimo, se possibile
    let goal = round2(line_total);
    let step = Decimal::new(1, 6);
    let mut best = price;
    for k in -50i64..=50 {
        let p = price + step * Decimal::from(k);
        if round2(apply_cascade(p, &disc) * qty) == goal {
            best = p;
            if k >= 0 {
                break;
            }
        }
    }
    Some(best)
}

/// Calcolo inverso del documento: sconto (o maggiorazione, se negativo) sul documento che
/// porta il totale IVA inclusa al valore indicato. Restituisce lo sconto e il totale ottenuto.
pub fn discount_for_total(doc: &Document, target: Decimal) -> Result<(Decimal, Decimal), CalcError> {
    let mut d = doc.clone();
    d.discount.clear();
    let base = compute(&d)?.total;
    if base.is_zero() {
        return Err(CalcError("Il documento non ha importi".into()));
    }
    if target <= Decimal::ZERO {
        return Err(CalcError("Il totale desiderato deve essere maggiore di zero".into()));
    }
    let total_with = |pct: Decimal| -> Result<Decimal, CalcError> {
        let mut x = doc.clone();
        x.discount = pct.normalize().to_string().replace('.', ",");
        Ok(compute(&x)?.total)
    };
    // lo sconto vale fino a 100, la maggiorazione fino a -100
    let (mut lo, mut hi) = (Decimal::from(-100), Decimal::from(9999) / Decimal::from(100));
    let step = Decimal::new(1, 4);
    let mut best = (Decimal::ZERO, base);
    for _ in 0..40 {
        if hi - lo < step {
            break;
        }
        let mid = round_dp((lo + hi) / Decimal::TWO, 4);
        let t = total_with(mid)?;
        if (t - target).abs() < (best.1 - target).abs() {
            best = (mid, t);
        }
        if t > target {
            lo = mid;
        } else if t < target {
            hi = mid;
        } else {
            return Ok((mid, t));
        }
    }
    for cand in [lo, hi] {
        let c = round_dp(cand, 4);
        let t = total_with(c)?;
        if (t - target).abs() < (best.1 - target).abs() {
            best = (c, t);
        }
    }
    Ok(best)
}

/// Ultimo giorno del mese di una data.
pub fn end_of_month(d: NaiveDate) -> NaiveDate {
    let (y, m) = if d.month() == 12 { (d.year() + 1, 1) } else { (d.year(), d.month() + 1) };
    NaiveDate::from_ymd_opt(y, m, 1).unwrap() - Duration::days(1)
}

/// Calcola le scadenze di un documento in base alla modalità di pagamento.
/// L'ultima rata assorbe le differenze di arrotondamento.
pub fn due_dates(doc_date: NaiveDate, amount: Decimal, payment: &PaymentMethod) -> Vec<DueDate> {
    let mut inst = payment.installments.clone();
    if inst.is_empty() {
        inst.push(Installment::default());
    }
    let mut out = Vec::new();
    let mut allocated = Decimal::ZERO;
    let n = inst.len();
    for (i, it) in inst.iter().enumerate() {
        let mut date = doc_date + Duration::days(it.days as i64);
        if it.end_of_month {
            date = end_of_month(date);
        }
        date += Duration::days(it.extra_days as i64);
        let part = if i + 1 == n { amount - allocated } else { round2(pct_of(amount, it.percent)) };
        allocated += part;
        out.push(DueDate {
            id: 0,
            date,
            amount: part,
            mp_code: if payment.mp_code.is_empty() { "MP05".into() } else { payment.mp_code.clone() },
            paid: Decimal::ZERO,
            writeoff: Decimal::ZERO,
            paid_date: None,
        });
    }
    out
}

/// Descrizione testuale delle rate: "30-60 gg DF FM".
pub fn describe_installments(p: &PaymentMethod) -> String {
    if p.installments.is_empty() || (p.installments.len() == 1 && p.installments[0].days == 0 && !p.installments[0].end_of_month) {
        return "Rimessa diretta".into();
    }
    let days: Vec<String> = p.installments.iter().map(|i| i.days.to_string()).collect();
    let mut s = format!("{} gg d.f.", days.join("-"));
    if p.installments.iter().any(|i| i.end_of_month) {
        s.push_str(" f.m.");
    }
    if let Some(e) = p.installments.iter().map(|i| i.extra_days).find(|d| *d > 0) {
        s.push_str(&format!(" +{}", e));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn vat(rate: Decimal, natura: Option<&str>) -> VatRef {
        VatRef { vat_id: None, rate, natura: natura.map(|s| s.to_string()), legal_ref: String::new(), split: false }
    }

    fn line(qty: Decimal, price: Decimal, disc: &str, v: VatRef) -> DocLine {
        DocLine { description: "x".into(), qty, unit_price: price, discounts: disc.into(), vat: v, ..Default::default() }
    }

    #[test]
    fn cascade_discount_sdi_example() {
        // Elenco controlli SdI, caso 2: 500 con sconti 8,50 + 5,15 + 3 su 100 pezzi
        let d = parse_discounts("8,50+5,15+3").unwrap();
        let unit = apply_cascade(dec!(500.00), &d);
        assert_eq!(unit, dec!(420.9205875));
        assert_eq!(round2(unit * dec!(100)), dec!(42092.06));
        assert_eq!(equivalent_discount(&parse_discounts("10+5").unwrap()), dec!(14.5));
    }

    #[test]
    fn fixture_f09_listino_sconti_successivi() {
        // F-09: listino 100, sconti 10% e 20% => 72
        let d = parse_discounts("10+20").unwrap();
        assert_eq!(apply_cascade(dec!(100), &d), dec!(72));
    }

    #[test]
    fn simple_invoice_f01() {
        let mut doc = Document::new(DocKind::Invoice);
        doc.lines.push(line(dec!(1), dec!(100), "", vat(dec!(22), None)));
        let t = compute(&doc).unwrap();
        assert_eq!(t.taxable, dec!(100.00));
        assert_eq!(t.tax, dec!(22.00));
        assert_eq!(t.total, dec!(122.00));
        assert_eq!(t.net_to_pay, dec!(122.00));
        assert!(t.stamp.is_none());
    }

    #[test]
    fn mixed_rates_and_rounding() {
        let mut doc = Document::new(DocKind::Invoice);
        doc.lines.push(line(dec!(3), dec!(10.3333), "", vat(dec!(22), None)));
        doc.lines.push(line(dec!(2.5), dec!(4.1234), "5", vat(dec!(10), None)));
        doc.lines.push(line(dec!(1), dec!(50), "", vat(dec!(0), Some("N4"))));
        let t = compute(&doc).unwrap();
        // 3 * 10.3333 = 30.9999 -> 31.00
        assert_eq!(t.lines[0].total, dec!(31.00));
        // 4.1234 * 0.95 = 3.91723 * 2.5 = 9.793075 -> 9.79
        assert_eq!(t.lines[1].total, dec!(9.79));
        assert_eq!(t.vat_rows.len(), 3);
        let r22 = t.vat_rows.iter().find(|r| r.rate == dec!(22)).unwrap();
        assert_eq!(r22.tax, dec!(6.82));
        let r10 = t.vat_rows.iter().find(|r| r.rate == dec!(10)).unwrap();
        assert_eq!(r10.tax, dec!(0.98));
        assert_eq!(t.total, dec!(31.00) + dec!(6.82) + dec!(9.79) + dec!(0.98) + dec!(50));
    }

    #[test]
    fn zero_rate_requires_nature() {
        let mut doc = Document::new(DocKind::Invoice);
        doc.lines.push(line(dec!(1), dec!(10), "", vat(dec!(0), None)));
        assert!(compute(&doc).is_err());
    }

    #[test]
    fn forfettario_with_stamp_and_inps() {
        // Forfettario: N2.2, rivalsa INPS 4% non soggetta a IVA, bollo 2 euro addebitato
        let mut doc = Document::new(DocKind::Invoice);
        let n22 = vat(dec!(0), Some("N2.2"));
        let mut l = line(dec!(1), dec!(1000), "", n22.clone());
        l.pension_fund = true;
        doc.lines.push(l);
        doc.pension_fund = Some(PensionFund { kind: "TC22".into(), rate: dec!(4), vat: n22, subject_to_withholding: false });
        let t = compute(&doc).unwrap();
        assert_eq!(t.pension_fund.as_ref().unwrap().amount, dec!(40.00));
        assert_eq!(t.stamp, Some(dec!(2.00)));
        assert!(t.stamp_charged);
        // N2.2: 1000 + 40 ; N1: 2 (bollo)
        assert_eq!(t.total, dec!(1042.00));
        assert_eq!(t.tax, dec!(0));
    }

    #[test]
    fn professional_with_withholding_and_cassa() {
        // Professionista ordinario: 1000 + cassa 4% (40) soggetta IVA 22%, ritenuta 20% su 1000
        let mut doc = Document::new(DocKind::Fee);
        let v22 = vat(dec!(22), None);
        let mut l = line(dec!(1), dec!(1000), "", v22.clone());
        l.pension_fund = true;
        l.withholding = true;
        doc.lines.push(l);
        doc.pension_fund = Some(PensionFund { kind: "TC04".into(), rate: dec!(4), vat: v22, subject_to_withholding: false });
        doc.withholdings.push(Withholding::default());
        let t = compute(&doc).unwrap();
        assert_eq!(t.taxable, dec!(1040.00));
        assert_eq!(t.tax, dec!(228.80));
        assert_eq!(t.total, dec!(1268.80));
        assert_eq!(t.withholding_total, dec!(200.00));
        assert_eq!(t.net_to_pay, dec!(1068.80));
    }

    #[test]
    fn split_payment() {
        let mut doc = Document::new(DocKind::Invoice);
        doc.vat_liability = "S".into();
        doc.lines.push(line(dec!(1), dec!(100), "", vat(dec!(22), None)));
        let t = compute(&doc).unwrap();
        assert_eq!(t.total, dec!(122.00));
        assert_eq!(t.net_to_pay, dec!(100.00));
        assert_eq!(t.vat_rows[0].liability, "S");
    }

    #[test]
    fn gross_prices() {
        let mut doc = Document::new(DocKind::Invoice);
        doc.prices_include_vat = true;
        doc.lines.push(line(dec!(3), dec!(9.99), "", vat(dec!(22), None)));
        let t = compute(&doc).unwrap();
        // totale ivato 29,97 => imponibile 24,57, imposta 5,41 (24,57+5,41 = 29,98: 1 cent)
        assert_eq!(t.taxable, dec!(24.57));
        assert_eq!(t.vat_rows[0].tax, dec!(5.41));
        let diff = (t.vat_rows[0].taxable - t.vat_rows[0].lines_total).abs();
        assert!(diff < dec!(1));
    }

    #[test]
    fn due_dates_eom() {
        let p = PaymentMethod {
            mp_code: "MP12".into(),
            installments: vec![
                Installment { days: 30, end_of_month: true, extra_days: 0, percent: dec!(50) },
                Installment { days: 60, end_of_month: true, extra_days: 10, percent: dec!(50) },
            ],
            ..Default::default()
        };
        let d = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let dd = due_dates(d, dec!(100.01), &p);
        assert_eq!(dd[0].date, NaiveDate::from_ymd_opt(2026, 2, 28).unwrap());
        assert_eq!(dd[1].date, NaiveDate::from_ymd_opt(2026, 4, 10).unwrap());
        assert_eq!(dd[0].amount + dd[1].amount, dec!(100.01));
        assert_eq!(describe_installments(&p), "30-60 gg d.f. f.m. +10");
    }

    #[test]
    fn document_discount_is_cascaded() {
        let mut doc = Document::new(DocKind::Invoice);
        doc.discount = "10".into();
        doc.lines.push(line(dec!(2), dec!(50), "10", vat(dec!(22), None)));
        let t = compute(&doc).unwrap();
        // 50 -10% = 45 -10% = 40.5 * 2 = 81
        assert_eq!(t.lines[0].total, dec!(81.00));
    }

    #[test]
    fn subtotals_calculated_lines_and_eco_fees() {
        let mut doc = Document::new(DocKind::Invoice);
        doc.lines.push(line(dec!(2), dec!(100), "", vat(dec!(22), None)));
        doc.lines.push(DocLine { eco_fee: dec!(1.50), eco_label: "Contributo RAEE".into(), ..line(dec!(1), dec!(50), "", vat(dec!(22), None)) });
        doc.lines.push(DocLine { kind: LineKind::Subtotal, ..Default::default() });
        doc.lines.push(DocLine { description: "Spese di trasporto 3%".into(), calc_percent: Some(dec!(3)), ..line(dec!(1), dec!(0), "", vat(dec!(22), None)) });
        doc.lines.push(DocLine { kind: LineKind::Subtotal, description: "Totale spese".into(), ..Default::default() });
        let t = compute(&doc).unwrap();
        // 200 + 50 + 1,50 RAEE
        let sub = t.lines.iter().find(|l| l.index == Some(2)).unwrap();
        assert_eq!(sub.subtotal, Some(dec!(251.50)));
        assert_eq!(sub.description, "Subtotale");
        let eco = t.lines.iter().find(|l| l.is_generated).unwrap();
        assert_eq!((eco.total, eco.description.as_str()), (dec!(1.50), "Contributo RAEE"));
        // 3% di 251,50 = 7,545 -> 7,55
        let pct = t.lines.iter().find(|l| l.index == Some(3)).unwrap();
        assert_eq!(pct.total, dec!(7.55));
        assert_eq!(t.lines.iter().find(|l| l.index == Some(4)).unwrap().subtotal, Some(dec!(7.55)));
        assert_eq!(t.taxable, dec!(259.05));
        // i subtotali non entrano nel riepilogo IVA
        assert_eq!(t.vat_rows.len(), 1);
        assert_eq!(t.vat_rows[0].taxable, dec!(259.05));
    }

    #[test]
    fn reverse_calculation() {
        // 3 pezzi, sconto 10, totale 100 IVA 22 inclusa
        let p = unit_price_for_total(dec!(100), dec!(3), "10", dec!(22), true, false).unwrap();
        let mut doc = Document::new(DocKind::Invoice);
        doc.lines.push(line(dec!(3), p, "10", vat(dec!(22), None)));
        let t = compute(&doc).unwrap();
        assert!((t.total - dec!(100)).abs() <= dec!(0.01), "{}", t.total);
        // prezzi ivati: il prezzo è ivato
        let p = unit_price_for_total(dec!(61), dec!(1), "", dec!(22), true, true).unwrap();
        assert_eq!(p, dec!(61));
        // sconto sul documento per arrivare a 1.000 IVA inclusa
        let mut doc = Document::new(DocKind::Invoice);
        doc.lines.push(line(dec!(1), dec!(900), "", vat(dec!(22), None)));
        doc.lines.push(line(dec!(2), dec!(50), "5", vat(dec!(10), None)));
        let (d, total) = discount_for_total(&doc, dec!(1000)).unwrap();
        assert!(d > dec!(0) && d < dec!(20), "{}", d);
        assert!((total - dec!(1000)).abs() <= dec!(0.05), "{}", total);
        // maggiorazione
        let (d, _) = discount_for_total(&doc, dec!(1300)).unwrap();
        assert!(d < Decimal::ZERO);
    }
}
