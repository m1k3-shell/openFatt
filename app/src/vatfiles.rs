//! Stampe dei registri IVA e della liquidazione, esportazione CSV, Comunicazione LIPE.

use chrono::NaiveDate;
use openfatt_core::money::fmt_eur;
use openfatt_core::vat::{self, Liquidation, Period, Register, RegisterRow, RetailRow};
use openfatt_pdf::table::{render_table, TableColumn, TableDoc, TableRow};
use openfatt_store::Store;
use rust_decimal::Decimal;

fn company_line(store: &Store) -> String {
    let c = store.company().unwrap_or_default();
    format!("{} - P.IVA {} - C.F. {}", c.display_name(), c.vat_number, c.tax_code)
}

fn period_line(from: NaiveDate, to: NaiveDate) -> String {
    format!("Periodo dal {} al {}", from.format("%d/%m/%Y"), to.format("%d/%m/%Y"))
}

fn row(cells: Vec<String>) -> TableRow {
    TableRow { cells, ..Default::default() }
}

fn summary_rows(rows: &[RegisterRow], ncols: usize, label_col: usize) -> Vec<TableRow> {
    let mut out = vec![TableRow { cells: { let mut c = vec![String::new(); ncols]; c[3] = "Riepilogo per aliquota".into(); c }, bold: true, shaded: true }];
    for a in vat::summary(rows) {
        let mut c = vec![String::new(); ncols];
        c[label_col] = format!("{}{}", a.label(), match a.liability.as_str() {
            "S" => " scissione pagamenti",
            "D" => " esigibilità differita",
            _ => "",
        });
        c[ncols - 2] = fmt_eur(a.taxable);
        c[ncols - 1] = fmt_eur(a.tax);
        out.push(row(c));
    }
    let mut c = vec![String::new(); ncols];
    c[label_col] = "Totale".into();
    c[ncols - 2] = fmt_eur(rows.iter().map(|r| r.taxable()).sum());
    c[ncols - 1] = fmt_eur(rows.iter().map(|r| r.tax()).sum());
    out.push(TableRow { cells: c, bold: true, shaded: false });
    out
}

/// Registro IVA vendite o acquisti in PDF.
pub fn register_pdf(store: &Store, register: Register, from: NaiveDate, to: NaiveDate, rows: &[RegisterRow]) -> Vec<u8> {
    let (title, columns) = match register {
        Register::Sales => (
            "Registro IVA delle fatture emesse",
            vec![
                TableColumn::new("Data", 1.0, false),
                TableColumn::new("Numero", 1.0, false),
                TableColumn::new("Tipo", 1.6, false),
                TableColumn::new("Cliente", 2.9, false),
                TableColumn::new("P.IVA / C.F.", 2.1, false),
                TableColumn::new("Aliquota", 1.0, false),
                TableColumn::new("Imponibile", 1.3, true),
                TableColumn::new("Imposta", 1.2, true),
            ],
        ),
        Register::Purchases => (
            "Registro IVA degli acquisti",
            vec![
                TableColumn::new("Registr.", 1.0, false),
                TableColumn::new("Prot.", 0.8, false),
                TableColumn::new("Doc. fornitore", 1.8, false),
                TableColumn::new("Fornitore", 2.7, false),
                TableColumn::new("P.IVA / C.F.", 2.1, false),
                TableColumn::new("Aliquota", 1.0, false),
                TableColumn::new("Imponibile", 1.3, true),
                TableColumn::new("Imposta", 1.2, true),
            ],
        ),
    };
    let mut table = Vec::new();
    for r in rows {
        for (i, a) in r.rates.iter().enumerate() {
            let rate = format!("{}{}", a.label(), if a.liability == "S" { " (S)" } else if a.liability == "D" { " (D)" } else { "" });
            let cells = if i > 0 {
                vec![String::new(), String::new(), String::new(), String::new(), String::new(), rate, fmt_eur(a.taxable), fmt_eur(a.tax)]
            } else {
                match register {
                    Register::Sales => vec![r.date.format("%d/%m/%Y").to_string(), r.number.clone(), format!("{} {}", r.kind_label, r.fe_type), r.party.clone(), r.party_vat.clone(), rate, fmt_eur(a.taxable), fmt_eur(a.tax)],
                    Register::Purchases => vec![
                        r.date.format("%d/%m/%Y").to_string(),
                        r.number.clone(),
                        format!("{} del {}", if r.external_number.is_empty() { "-" } else { r.external_number.as_str() }, r.doc_date.format("%d/%m/%Y")),
                        if r.reverse_charge { format!("{} (integrazione)", r.party) } else { r.party.clone() },
                        r.party_vat.clone(),
                        rate,
                        fmt_eur(a.taxable),
                        fmt_eur(a.tax),
                    ],
                }
            };
            table.push(row(cells));
        }
    }
    let counted: Vec<RegisterRow> = rows.iter().filter(|r| register == Register::Sales || !r.reverse_charge).cloned().collect();
    table.extend(summary_rows(&counted, 8, 5));
    let mut sub = vec![company_line(store), period_line(from, to)];
    if register == Register::Purchases && rows.iter().any(|r| r.reverse_charge) {
        sub.push("Le integrazioni in reverse charge sono riportate anche qui; il riepilogo le esclude (sono nel registro delle vendite).".into());
    }
    render_table(&TableDoc { title: title.into(), subtitle: sub, columns, rows: table, footer: "(S) scissione dei pagamenti - (D) esigibilità differita".into(), page_prefix: format!("{}/", from.format("%Y")) })
}

/// Registro dei corrispettivi in PDF.
pub fn retail_pdf(store: &Store, from: NaiveDate, to: NaiveDate, rows: &[RetailRow]) -> Vec<u8> {
    let columns = vec![TableColumn::new("Data", 1.0, false), TableColumn::new("Vendite", 0.8, true), TableColumn::new("Aliquota", 1.0, false), TableColumn::new("Imponibile", 1.3, true), TableColumn::new("Imposta", 1.2, true), TableColumn::new("Corrispettivo", 1.4, true)];
    let mut table = Vec::new();
    let mut totals: std::collections::BTreeMap<String, (Decimal, Decimal)> = Default::default();
    for d in rows {
        for (i, (l, taxable, tax)) in d.rates.iter().enumerate() {
            let e = totals.entry(l.clone()).or_default();
            e.0 += *taxable;
            e.1 += *tax;
            table.push(row(vec![
                if i == 0 { d.date.format("%d/%m/%Y").to_string() } else { String::new() },
                if i == 0 { d.count.to_string() } else { String::new() },
                l.clone(),
                fmt_eur(*taxable),
                fmt_eur(*tax),
                fmt_eur(*taxable + *tax),
            ]));
        }
    }
    table.push(TableRow { cells: vec![String::new(), String::new(), "Riepilogo".into()], bold: true, shaded: true });
    for (l, (a, b)) in &totals {
        table.push(row(vec![String::new(), String::new(), l.clone(), fmt_eur(*a), fmt_eur(*b), fmt_eur(*a + *b)]));
    }
    render_table(&TableDoc { title: "Registro dei corrispettivi".into(), subtitle: vec![company_line(store), period_line(from, to)], columns, rows: table, footer: String::new(), page_prefix: format!("{}/", from.format("%Y")) })
}

/// Righe del prospetto di liquidazione: (rigo, descrizione, debito, credito).
pub fn liquidation_lines(l: &Liquidation) -> Vec<(&'static str, &'static str, Decimal, Decimal)> {
    vec![
        ("VP2", "Totale operazioni attive (al netto dell'IVA)", l.totals.sales_taxable, Decimal::ZERO),
        ("VP3", "Totale operazioni passive (al netto dell'IVA)", l.totals.purchases_taxable, Decimal::ZERO),
        ("VP4", "IVA esigibile", l.totals.vat_due, Decimal::ZERO),
        ("VP5", "IVA detratta", Decimal::ZERO, l.totals.vat_deducted),
        ("VP6", "IVA dovuta o a credito", l.vat_payable, l.vat_credit),
        ("VP7", "Debito del periodo precedente non superiore alla soglia", l.prev_debt, Decimal::ZERO),
        ("VP8", "Credito del periodo precedente", Decimal::ZERO, l.prev_credit),
        ("VP9", "Credito dell'anno precedente", Decimal::ZERO, l.inputs.prev_year_credit),
        ("VP10", "Versamenti auto UE (F24 elementi identificativi)", Decimal::ZERO, l.inputs.car_payments),
        ("VP11", "Crediti d'imposta", Decimal::ZERO, l.inputs.tax_credits),
        ("VP12", "Interessi dovuti per liquidazioni trimestrali", l.interest, Decimal::ZERO),
        ("VP13", "Acconto dovuto", Decimal::ZERO, l.inputs.advance),
        ("VP14", "IVA da versare o a credito", l.to_pay, l.credit),
    ]
}

pub fn liquidation_pdf(store: &Store, p: Period, l: &Liquidation) -> Vec<u8> {
    let columns = vec![TableColumn::new("Rigo", 0.7, false), TableColumn::new("Descrizione", 5.0, false), TableColumn::new("Importo / debito", 1.5, true), TableColumn::new("Credito", 1.3, true)];
    let money = |d: Decimal| if d.is_zero() { String::new() } else { fmt_eur(d) };
    let mut rows: Vec<TableRow> = liquidation_lines(l).into_iter().map(|(r, d, a, b)| TableRow { cells: vec![r.into(), d.into(), money(a), money(b)], bold: r == "VP14" || r == "VP6", shaded: false }).collect();
    rows.push(TableRow::default());
    rows.push(TableRow { cells: vec![String::new(), l.outcome(p)], bold: true, shaded: true });
    if !l.totals.split_payment.is_zero() {
        rows.push(row(vec![String::new(), format!("IVA in scissione dei pagamenti, versata dalla PA: € {}", fmt_eur(l.totals.split_payment))]));
    }
    if !l.totals.deferred_pending.is_zero() {
        rows.push(row(vec![String::new(), format!("IVA ad esigibilità differita sorta nel periodo, esigibile all'incasso: € {}", fmt_eur(l.totals.deferred_pending))]));
    }
    render_table(&TableDoc {
        title: format!("Liquidazione IVA - {}", p.label()),
        subtitle: vec![company_line(store), period_line(p.first(), p.last())],
        columns,
        rows,
        footer: "Prospetto secondo i righi del quadro VP della Comunicazione liquidazioni periodiche IVA".into(),
        page_prefix: String::new(),
    })
}

fn csv_field(s: &str) -> String {
    if s.contains([';', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Registro in CSV (separatore ";", importi con la virgola: si apre in Excel).
pub fn register_csv(register: Register, rows: &[RegisterRow]) -> String {
    let mut s = String::from("\u{feff}");
    s.push_str(match register {
        Register::Sales => "Data;Numero;Tipo;TD;Cliente;P.IVA/C.F.;Aliquota/Natura;Esigibilita;Imponibile;Imposta;Totale documento\r\n",
        Register::Purchases => "Data registrazione;Protocollo;Numero fornitore;Data documento;Fornitore;P.IVA/C.F.;Aliquota/Natura;Esigibilita;Imponibile;Imposta;Totale documento;Integrazione\r\n",
    });
    let n = |d: Decimal| format!("{:.2}", d).replace('.', ",");
    for r in rows {
        for a in &r.rates {
            let cols: Vec<String> = match register {
                Register::Sales => vec![r.date.format("%d/%m/%Y").to_string(), r.number.clone(), r.kind_label.clone(), r.fe_type.clone(), r.party.clone(), r.party_vat.clone(), a.label(), a.liability.clone(), n(a.taxable), n(a.tax), n(r.total)],
                Register::Purchases => vec![
                    r.date.format("%d/%m/%Y").to_string(),
                    r.number.clone(),
                    r.external_number.clone(),
                    r.doc_date.format("%d/%m/%Y").to_string(),
                    r.party.clone(),
                    r.party_vat.clone(),
                    a.label(),
                    a.liability.clone(),
                    n(a.taxable),
                    n(a.tax),
                    n(r.total),
                    if r.reverse_charge { "si".into() } else { String::new() },
                ],
            };
            s.push_str(&cols.iter().map(|c| csv_field(c)).collect::<Vec<_>>().join(";"));
            s.push_str("\r\n");
        }
    }
    s
}

/// Comunicazione LIPE del trimestre `quarter`: nome del file e contenuto XML.
pub fn lipe(store: &Store, year: i32, quarter: u32) -> Result<(String, String), Vec<String>> {
    let s = store.vat_settings().map_err(|e| vec![e.to_string()])?;
    let c = store.company().map_err(|e| vec![e.to_string()])?;
    let last = Period::new(year, if s.periodicity == vat::Periodicity::Quarterly { quarter } else { quarter * 3 }, s.periodicity);
    let all = store.vat_liquidations(year, last.index).map_err(|e| vec![e.to_string()])?;
    let modules: Vec<(Period, Liquidation)> = all.into_iter().filter(|(p, _)| p.quarter() == quarter).collect();
    let cf = if c.tax_code.trim().is_empty() { c.vat_number.trim().to_string() } else { c.tax_code.trim().to_string() };
    let subject = vat::LipeSubject { tax_code: cf.clone(), vat_number: c.vat_number.trim().to_string(), declarant_cf: s.declarant_cf.clone(), declarant_role: s.declarant_role };
    let xml = vat::lipe_xml(&subject, year, &modules)?;
    let n = store.next_counter("lipe").map_err(|e| vec![e.to_string()])?;
    Ok((vat::lipe_file_name(&cf, n), xml))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stampe_e_comunicazione_dall_archivio_dimostrativo() {
        let s = Store::open_in_memory().unwrap();
        crate::demo::populate(&s).unwrap();
        let today = chrono::Local::now().date_naive();
        let year = today.format("%Y").to_string().parse::<i32>().unwrap();
        let (from, to) = (NaiveDate::from_ymd_opt(year, 1, 1).unwrap(), NaiveDate::from_ymd_opt(year, 12, 31).unwrap());
        let sales = s.vat_register(Register::Sales, from, to).unwrap();
        assert!(!sales.is_empty());
        let pdf = register_pdf(&s, Register::Sales, from, to, &sales);
        assert!(pdf.starts_with(b"%PDF"));
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/pdf-samples");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("registro-vendite.pdf"), &pdf).unwrap();
        let purchases = s.vat_register(Register::Purchases, from, to).unwrap();
        assert!(!purchases.is_empty());
        assert!(register_pdf(&s, Register::Purchases, from, to, &purchases).starts_with(b"%PDF"));
        assert!(register_csv(Register::Sales, &sales).lines().count() > 1);
        assert!(retail_pdf(&s, from, to, &s.retail_register(from, to).unwrap()).starts_with(b"%PDF"));
        let q = Period::containing(today, openfatt_core::vat::Periodicity::Monthly);
        let y = s.vat_liquidations(year, q.index).unwrap();
        let lp = liquidation_pdf(&s, y[y.len() - 2].0, &y[y.len() - 2].1);
        assert!(lp.starts_with(b"%PDF"));
        std::fs::write(out.join("liquidazione.pdf"), &lp).unwrap();
        // una società ha bisogno del dichiarante (la demo lo ha: lo si toglie per la prova)
        let quarter = q.quarter();
        let mut st = s.vat_settings().unwrap();
        st.declarant_cf.clear();
        s.set_vat_settings(&st).unwrap();
        let err = lipe(&s, year, quarter).unwrap_err();
        assert!(err.iter().any(|e| e.contains("dichiarante")), "{:?}", err);
        let mut st = s.vat_settings().unwrap();
        st.declarant_cf = "RSSMRA85T10A562S".into();
        s.set_vat_settings(&st).unwrap();
        let (name, xml) = lipe(&s, year, quarter).unwrap();
        assert!(name.starts_with("IT12345678903_LI_"));
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/xsd-samples/lipe");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lipe-demo.xml"), xml).unwrap();
    }
}
