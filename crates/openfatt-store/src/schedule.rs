//! Report programmati: all'apertura dell'archivio si preparano da soli, nella cartella
//! scelta, i report arrivati a scadenza (settimanali o mensili, sul periodo appena chiuso).

use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::{Result, Store};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ReportKind {
    /// Prima nota e registri IVA in CSV con i codici dello studio.
    #[default]
    Accountant,
    /// Statistiche delle vendite per cliente (Excel).
    SalesByCustomer,
    /// Statistiche per articolo con classi ABC (Excel).
    SalesByProduct,
    /// Scadenze aperte (CSV).
    OpenDues,
}

impl ReportKind {
    pub const ALL: [ReportKind; 4] = [ReportKind::Accountant, ReportKind::SalesByCustomer, ReportKind::SalesByProduct, ReportKind::OpenDues];
    pub fn label(self) -> &'static str {
        match self {
            ReportKind::Accountant => "Prima nota e registri IVA per lo studio (CSV)",
            ReportKind::SalesByCustomer => "Vendite per cliente (Excel)",
            ReportKind::SalesByProduct => "Vendite per articolo con ABC (Excel)",
            ReportKind::OpenDues => "Scadenze aperte (CSV)",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Every {
    Weekly,
    #[default]
    Monthly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ScheduledReport {
    pub kind: ReportKind,
    pub every: Every,
    pub folder: String,
    pub enabled: bool,
    /// Ultimo periodo preparato (fine del periodo).
    pub last_period_end: Option<NaiveDate>,
}

/// Periodo chiuso più recente alla data: settimana da lunedì a domenica, oppure mese.
pub fn last_closed_period(every: Every, today: NaiveDate) -> (NaiveDate, NaiveDate) {
    match every {
        Every::Weekly => {
            let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
            (monday - Duration::days(7), monday - Duration::days(1))
        }
        Every::Monthly => {
            let first = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap();
            let end = first - Duration::days(1);
            (NaiveDate::from_ymd_opt(end.year(), end.month(), 1).unwrap(), end)
        }
    }
}

impl ScheduledReport {
    /// Periodo da preparare oggi, se non è già stato fatto.
    pub fn due(&self, today: NaiveDate) -> Option<(NaiveDate, NaiveDate)> {
        if !self.enabled || self.folder.trim().is_empty() {
            return None;
        }
        let (from, to) = last_closed_period(self.every, today);
        match self.last_period_end {
            Some(done) if done >= to => None,
            _ => Some((from, to)),
        }
    }
}

impl Store {
    pub fn scheduled_reports(&self) -> Result<Vec<ScheduledReport>> {
        self.get_setting("scheduled_reports")
    }

    pub fn set_scheduled_reports(&self, v: &[ScheduledReport]) -> Result<()> {
        self.set_setting("scheduled_reports", &v)
    }

    /// Segna come preparato il periodo che finisce a `to` per il report in posizione `index`.
    pub fn mark_report_done(&self, index: usize, to: NaiveDate) -> Result<()> {
        let mut v = self.scheduled_reports()?;
        if let Some(r) = v.get_mut(index) {
            r.last_period_end = Some(to);
        }
        self.set_scheduled_reports(&v)
    }

    /// Scadenze aperte in CSV (per i report programmati).
    pub fn open_dues_csv(&self) -> Result<String> {
        let mut out = String::from("Scadenza;Documento;Soggetto;Da incassare;Da pagare\r\n");
        for d in self.due_list(true, None)? {
            let r = d.signed_residual();
            let (inc, pay) = if d.customer_side() { (r, rust_decimal::Decimal::ZERO) } else { (rust_decimal::Decimal::ZERO, r) };
            let f = |x: rust_decimal::Decimal| if x.is_zero() { String::new() } else { format!("{:.2}", x).replace('.', ",") };
            out.push_str(&format!("{};{};{};{};{}\r\n", d.date.format("%d/%m/%Y"), d.doc_label.replace(';', ","), d.party_name.replace(';', ","), f(inc), f(pay)));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }

    #[test]
    fn periodi_e_scadenze_dei_report() {
        // sabato 28 novembre 2026: settimana chiusa 16-22, mese chiuso ottobre
        assert_eq!(last_closed_period(Every::Weekly, d(2026, 11, 28)), (d(2026, 11, 16), d(2026, 11, 22)));
        assert_eq!(last_closed_period(Every::Monthly, d(2026, 11, 28)), (d(2026, 10, 1), d(2026, 10, 31)));
        assert_eq!(last_closed_period(Every::Monthly, d(2027, 1, 3)), (d(2026, 12, 1), d(2026, 12, 31)));
        let mut r = ScheduledReport { kind: ReportKind::Accountant, every: Every::Monthly, folder: "cartella".into(), enabled: true, last_period_end: None };
        assert_eq!(r.due(d(2026, 11, 28)), Some((d(2026, 10, 1), d(2026, 10, 31))));
        r.last_period_end = Some(d(2026, 10, 31));
        assert_eq!(r.due(d(2026, 11, 30)), None);
        assert_eq!(r.due(d(2026, 12, 1)), Some((d(2026, 11, 1), d(2026, 11, 30))));
        r.enabled = false;
        assert_eq!(r.due(d(2026, 12, 1)), None);
        let s = Store::open_in_memory().unwrap();
        s.set_scheduled_reports(&[ScheduledReport { enabled: true, folder: "x".into(), ..Default::default() }]).unwrap();
        s.mark_report_done(0, d(2026, 10, 31)).unwrap();
        assert_eq!(s.scheduled_reports().unwrap()[0].last_period_end, Some(d(2026, 10, 31)));
        assert!(s.open_dues_csv().unwrap().starts_with("Scadenza;"));
    }
}
