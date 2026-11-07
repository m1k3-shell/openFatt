//! Esportazione in Excel (.xlsx) di un elenco qualsiasi: intestazione bloccata, filtri
//! automatici, numeri e date come valori veri (si possono sommare e ordinare in Excel),
//! riga dei totali.

use std::path::Path;

use chrono::{Datelike, NaiveDate};

#[derive(Debug, Clone, PartialEq)]
pub enum XValue {
    Text(String),
    /// Numero con i decimali da mostrare.
    Number(f64, u32),
    /// Importo in euro (2 decimali).
    Money(f64),
    Date(NaiveDate),
    Bool(bool),
    Empty,
}

#[derive(Debug, Clone)]
pub struct XColumn {
    pub title: String,
    /// Larghezza in caratteri.
    pub width: f64,
    /// Somma nella riga dei totali.
    pub total: bool,
}

pub fn export_table(path: &Path, sheet: &str, title: &str, columns: &[XColumn], rows: &[Vec<XValue>]) -> Result<(), String> {
    use rust_xlsxwriter::{ExcelDateTime, Format, FormatBorder, Workbook};
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let name: String = sheet.chars().filter(|c| !"[]:*?/\\".contains(*c)).take(31).collect();
    ws.set_name(if name.is_empty() { "Elenco" } else { &name }).map_err(|e| e.to_string())?;
    let bold = Format::new().set_bold();
    let head = Format::new().set_bold().set_background_color(rust_xlsxwriter::Color::RGB(0xE4E7EC)).set_border_bottom(FormatBorder::Thin);
    let money = Format::new().set_num_format("#,##0.00");
    let date = Format::new().set_num_format("dd/mm/yyyy");
    let total_money = Format::new().set_bold().set_num_format("#,##0.00").set_border_top(FormatBorder::Thin);
    let e = |r: Result<&mut rust_xlsxwriter::Worksheet, rust_xlsxwriter::XlsxError>| r.map(|_| ()).map_err(|x| x.to_string());
    e(ws.write_string_with_format(0, 0, title, &bold))?;
    let hr = 2u32;
    for (c, col) in columns.iter().enumerate() {
        e(ws.write_string_with_format(hr, c as u16, &col.title, &head))?;
        ws.set_column_width(c as u16, col.width.clamp(6.0, 60.0)).map_err(|x| x.to_string())?;
    }
    let mut num_formats: Vec<Option<Format>> = vec![None; columns.len()];
    for (r, row) in rows.iter().enumerate() {
        let rr = hr + 1 + r as u32;
        for (c, v) in row.iter().enumerate().take(columns.len()) {
            let cc = c as u16;
            match v {
                XValue::Text(s) => e(ws.write_string(rr, cc, s))?,
                XValue::Number(n, dp) => {
                    let f = num_formats[c].get_or_insert_with(|| Format::new().set_num_format(if *dp == 0 { "#,##0".to_string() } else { format!("#,##0.{}", "0".repeat(*dp as usize)) }));
                    e(ws.write_number_with_format(rr, cc, *n, f))?
                }
                XValue::Money(n) => e(ws.write_number_with_format(rr, cc, *n, &money))?,
                XValue::Date(d) => {
                    let dt = ExcelDateTime::from_ymd(d.year() as u16, d.month() as u8, d.day() as u8).map_err(|x| x.to_string())?;
                    e(ws.write_datetime_with_format(rr, cc, &dt, &date))?
                }
                XValue::Bool(b) => e(ws.write_string(rr, cc, if *b { "sì" } else { "no" }))?,
                XValue::Empty => {}
            }
        }
    }
    let last = hr + rows.len() as u32;
    if !rows.is_empty() {
        ws.autofilter(hr, 0, last, columns.len().saturating_sub(1) as u16).map_err(|x| x.to_string())?;
        if columns.iter().any(|c| c.total) {
            let tr = last + 1;
            e(ws.write_string_with_format(tr, 0, "Totale", &bold))?;
            for (c, _) in columns.iter().enumerate().filter(|(_, c)| c.total) {
                let letter = rust_xlsxwriter::utility::column_number_to_name(c as u16);
                let formula = format!("=SUBTOTAL(9,{l}{a}:{l}{b})", l = letter, a = hr + 2, b = last + 1);
                e(ws.write_formula_with_format(tr, c as u16, formula.as_str(), &total_money))?;
            }
        }
    }
    ws.set_freeze_panes(hr + 1, 0).map_err(|x| x.to_string())?;
    wb.save(path).map_err(|x| format!("Salvataggio di {} non riuscito: {}", path.display(), x))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esporta_e_rilegge() {
        let dir = std::env::temp_dir().join("openfatt-interop-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("elenco.xlsx");
        let cols = vec![
            XColumn { title: "Data".into(), width: 12.0, total: false },
            XColumn { title: "Cliente".into(), width: 30.0, total: false },
            XColumn { title: "Totale".into(), width: 14.0, total: true },
            XColumn { title: "Quantità".into(), width: 10.0, total: false },
        ];
        let rows = vec![
            vec![XValue::Date(NaiveDate::from_ymd_opt(2026, 3, 1).unwrap()), XValue::Text("Rossi S.p.A.".into()), XValue::Money(1220.5), XValue::Number(3.0, 0)],
            vec![XValue::Date(NaiveDate::from_ymd_opt(2026, 3, 2).unwrap()), XValue::Text("Città & Co.".into()), XValue::Money(80.0), XValue::Empty],
        ];
        export_table(&p, "Fatture", "Fatture di marzo", &cols, &rows).unwrap();
        use calamine::{open_workbook_auto, Data, Reader};
        let mut wb = open_workbook_auto(&p).unwrap();
        let r = wb.worksheet_range("Fatture").unwrap();
        assert_eq!(r.get_value((2, 1)), Some(&Data::String("Cliente".into())));
        assert_eq!(r.get_value((3, 2)), Some(&Data::Float(1220.5)));
        assert_eq!(r.get_value((4, 1)), Some(&Data::String("Città & Co.".into())));
        assert!(matches!(r.get_value((3, 0)), Some(Data::DateTime(_))));
        assert_eq!(r.get_value((5, 0)), Some(&Data::String("Totale".into())));
    }
}
