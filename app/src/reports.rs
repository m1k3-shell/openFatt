//! Stampe e esportazioni dei report: estratto provvigioni, statistiche, inventario di fine
//! anno (valore del magazzino al 31/12).

use std::path::Path;

use chrono::NaiveDate;
use openfatt_core::money::{fmt_it, fmt_it_trim};
use openfatt_interop::table::{export_table, XColumn, XValue};
use openfatt_pdf::table::{render_table, TableColumn, TableDoc, TableRow};
use openfatt_store::{Agent, CommissionRow, StatRow, Store};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

fn f(d: Decimal) -> f64 {
    d.to_f64().unwrap_or(0.0)
}

fn company_line(store: &Store) -> String {
    let c = store.company().unwrap_or_default();
    format!("{}  ·  P.IVA {}", c.display_name(), c.vat_number)
}

pub fn commissions_pdf(store: &Store, agent: &Agent, from: NaiveDate, to: NaiveDate, rows: &[CommissionRow]) -> Result<Vec<u8>, String> {
    let mut t = Vec::new();
    for r in rows {
        t.push(TableRow {
            cells: vec![r.date.format("%d/%m/%Y").to_string(), r.title.clone(), r.party.clone(), fmt_it(r.base, 2), fmt_it(r.commission, 2), format!("{}%", fmt_it_trim(r.collected * Decimal::ONE_HUNDRED, 0, 0)), fmt_it(r.matured, 2), fmt_it(r.settled, 2), fmt_it(r.to_settle(), 2)],
            ..Default::default()
        });
    }
    let sum = |g: fn(&CommissionRow) -> Decimal| fmt_it(rows.iter().map(g).sum::<Decimal>(), 2);
    t.push(TableRow { cells: vec!["".into(), "Totale".into(), "".into(), sum(|r| r.base), sum(|r| r.commission), "".into(), sum(|r| r.matured), sum(|r| r.settled), sum(|r| r.to_settle())], bold: true, shaded: true });
    let doc = TableDoc {
        title: format!("Estratto conto provvigioni - {}", agent.name),
        subtitle: vec![company_line(store), format!("Periodo dal {} al {} · provvigione {}% {}", from.format("%d/%m/%Y"), to.format("%d/%m/%Y"), fmt_it_trim(agent.rate, 0, 2), if agent.on_collected { "sull'incassato" } else { "sul fatturato" })],
        columns: vec![
            TableColumn::new("Data", 1.0, false),
            TableColumn::new("Documento", 2.6, false),
            TableColumn::new("Cliente", 2.6, false),
            TableColumn::new("Imponibile", 1.3, true),
            TableColumn::new("Provvigione", 1.2, true),
            TableColumn::new("Incass.", 0.8, true),
            TableColumn::new("Maturata", 1.2, true),
            TableColumn::new("Liquidata", 1.2, true),
            TableColumn::new("Da liquidare", 1.3, true),
        ],
        rows: t,
        footer: "Importi in euro".into(),
        page_prefix: String::new(),
    };
    Ok(render_table(&doc))
}

pub fn commissions_xlsx(path: &Path, agent: &Agent, rows: &[CommissionRow]) -> Result<(), String> {
    let cols = vec![
        XColumn { title: "Data".into(), width: 11.0, total: false },
        XColumn { title: "Documento".into(), width: 34.0, total: false },
        XColumn { title: "Cliente".into(), width: 30.0, total: false },
        XColumn { title: "Imponibile".into(), width: 13.0, total: true },
        XColumn { title: "Provvigione".into(), width: 13.0, total: true },
        XColumn { title: "Incassato %".into(), width: 11.0, total: false },
        XColumn { title: "Maturata".into(), width: 13.0, total: true },
        XColumn { title: "Liquidata".into(), width: 13.0, total: true },
        XColumn { title: "Insoluti".into(), width: 13.0, total: true },
    ];
    let data: Vec<Vec<XValue>> = rows
        .iter()
        .map(|r| vec![XValue::Date(r.date), XValue::Text(r.title.clone()), XValue::Text(r.party.clone()), XValue::Money(f(r.base)), XValue::Money(f(r.commission)), XValue::Number(f(r.collected * Decimal::ONE_HUNDRED), 0), XValue::Money(f(r.matured)), XValue::Money(f(r.settled)), XValue::Money(f(r.overdue))])
        .collect();
    export_table(path, "Provvigioni", &format!("Provvigioni {}", agent.name), &cols, &data)
}

pub fn stats_xlsx(path: &Path, title: &str, rows: &[StatRow]) -> Result<(), String> {
    let cols = vec![
        XColumn { title: "Voce".into(), width: 44.0, total: false },
        XColumn { title: "Quantità".into(), width: 12.0, total: true },
        XColumn { title: "Venduto".into(), width: 14.0, total: true },
        XColumn { title: "Costo".into(), width: 14.0, total: true },
        XColumn { title: "Margine".into(), width: 14.0, total: true },
        XColumn { title: "Margine %".into(), width: 10.0, total: false },
        XColumn { title: "Documenti".into(), width: 10.0, total: true },
    ];
    let data: Vec<Vec<XValue>> = rows
        .iter()
        .map(|r| vec![XValue::Text(r.label.clone()), XValue::Number(f(r.qty), 2), XValue::Money(f(r.revenue)), XValue::Money(f(r.cost)), XValue::Money(f(r.margin())), r.margin_pct().map(|m| XValue::Number(f(m), 1)).unwrap_or(XValue::Empty), XValue::Number(r.documents as f64, 0)])
        .collect();
    export_table(path, "Statistica", title, &cols, &data)
}

/// Inventario di fine anno: giacenze e valore al costo al 31/12 (per il libro degli inventari).
pub fn inventory_valuation(store: &Store, year: i32) -> Result<(TableDoc, Vec<Vec<XValue>>), String> {
    let to = NaiveDate::from_ymd_opt(year, 12, 31).unwrap();
    let moves = store.stock_moves(None, usize::MAX).map_err(|e| e.to_string())?;
    let mut qty: std::collections::BTreeMap<i64, Decimal> = std::collections::BTreeMap::new();
    for m in moves.iter().filter(|m| m.date <= to) {
        *qty.entry(m.product_id).or_default() += m.qty;
    }
    let mut items: Vec<(String, String, String, Decimal, Decimal)> = Vec::new();
    for (pid, q) in qty {
        if q <= Decimal::ZERO {
            continue;
        }
        if let Ok(Some(p)) = store.product(pid) {
            items.push((p.code, p.description, p.unit, q, p.cost));
        }
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let total: Decimal = items.iter().map(|i| i.3 * i.4).sum();
    let mut rows: Vec<TableRow> = items.iter().map(|i| TableRow { cells: vec![i.0.clone(), i.1.clone(), i.2.clone(), fmt_it_trim(i.3, 0, 3), fmt_it(i.4, 4), fmt_it(i.3 * i.4, 2)], ..Default::default() }).collect();
    rows.push(TableRow { cells: vec!["".into(), format!("Totale ({} articoli)", items.len()), "".into(), "".into(), "".into(), fmt_it(total, 2)], bold: true, shaded: true });
    let doc = TableDoc {
        title: format!("Inventario delle merci al 31/12/{}", year),
        subtitle: vec![company_line(store), "Valorizzazione al costo di acquisto dell'anagrafica articoli".into()],
        columns: vec![TableColumn::new("Codice", 1.4, false), TableColumn::new("Descrizione", 4.0, false), TableColumn::new("U.M.", 0.6, false), TableColumn::new("Quantità", 1.1, true), TableColumn::new("Costo unit.", 1.2, true), TableColumn::new("Valore", 1.4, true)],
        rows,
        footer: format!("Inventario {}", year),
        page_prefix: format!("{}/", year),
    };
    let x: Vec<Vec<XValue>> = items.iter().map(|i| vec![XValue::Text(i.0.clone()), XValue::Text(i.1.clone()), XValue::Text(i.2.clone()), XValue::Number(f(i.3), 3), XValue::Number(f(i.4), 4), XValue::Money(f(i.3 * i.4))]).collect();
    Ok((doc, x))
}

pub fn inventory_valuation_pdf(store: &Store, year: i32) -> Result<Vec<u8>, String> {
    Ok(render_table(&inventory_valuation(store, year)?.0))
}

pub fn inventory_valuation_xlsx(store: &Store, year: i32, path: &Path) -> Result<(), String> {
    let (_, rows) = inventory_valuation(store, year)?;
    let cols = vec![
        XColumn { title: "Codice".into(), width: 16.0, total: false },
        XColumn { title: "Descrizione".into(), width: 44.0, total: false },
        XColumn { title: "U.M.".into(), width: 6.0, total: false },
        XColumn { title: "Quantità".into(), width: 12.0, total: true },
        XColumn { title: "Costo unitario".into(), width: 14.0, total: false },
        XColumn { title: "Valore".into(), width: 14.0, total: true },
    ];
    export_table(path, "Inventario", &format!("Inventario al 31/12/{}", year), &cols, &rows)
}

/// Prospetto della dichiarazione IVA annuale (PDF).
pub fn annual_vat_pdf(store: &Store, a: &openfatt_store::AnnualVat) -> Result<Vec<u8>, String> {
    let mut rows = Vec::new();
    for q in &a.quadri {
        rows.push(TableRow { cells: vec![q.code.clone(), format!("QUADRO {} - {}", q.code, q.title), String::new(), String::new()], bold: true, shaded: true });
        for r in &q.righi {
            if r.values.len() <= 2 {
                let v: Vec<String> = r.values.iter().map(|(t, v)| format!("{} {}", t, fmt_it(*v, 2))).collect();
                let (a1, a2) = match v.len() {
                    0 => (String::new(), String::new()),
                    1 => (String::new(), v[0].clone()),
                    _ => (v[0].clone(), v[1].clone()),
                };
                rows.push(TableRow { cells: vec![r.code.clone(), r.label.clone(), a1, a2], ..Default::default() });
            } else {
                // righi con molte colonne: una riga per colonna
                rows.push(TableRow { cells: vec![r.code.clone(), r.label.clone(), String::new(), String::new()], ..Default::default() });
                for (t, v) in &r.values {
                    rows.push(TableRow { cells: vec![String::new(), format!("    {}", t), String::new(), fmt_it(*v, 2)], ..Default::default() });
                }
            }
        }
    }
    if !a.warnings.is_empty() {
        rows.push(TableRow { cells: vec!["".into(), "Da verificare".into(), "".into(), "".into()], bold: true, shaded: true });
        for w in &a.warnings {
            // gli avvisi lunghi vanno su più righe
            let mut line = String::new();
            for word in w.split(' ') {
                if line.chars().count() + word.chars().count() > 95 {
                    rows.push(TableRow { cells: vec!["".into(), std::mem::take(&mut line), "".into(), "".into()], ..Default::default() });
                }
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
            }
            rows.push(TableRow { cells: vec!["".into(), line, "".into(), "".into()], ..Default::default() });
        }
    }
    let doc = TableDoc {
        title: format!("Prospetto per la dichiarazione IVA {} - anno d'imposta {}", a.year + 1, a.year),
        subtitle: vec![company_line(store), "Valori calcolati dai registri IVA e dalle liquidazioni periodiche; righi secondo le istruzioni ufficiali del modello IVA".into()],
        columns: vec![TableColumn::new("Rigo", 0.8, false), TableColumn::new("Descrizione", 6.2, false), TableColumn::new("", 2.0, true), TableColumn::new("", 2.0, true)],
        rows,
        footer: "Prospetto di lavoro per il commercialista: non è la dichiarazione".into(),
        page_prefix: String::new(),
    };
    Ok(render_table(&doc))
}

pub fn annual_vat_xlsx(path: &Path, a: &openfatt_store::AnnualVat) -> Result<(), String> {
    let cols = vec![
        XColumn { title: "Quadro".into(), width: 8.0, total: false },
        XColumn { title: "Rigo".into(), width: 8.0, total: false },
        XColumn { title: "Descrizione".into(), width: 60.0, total: false },
        XColumn { title: "Colonna".into(), width: 24.0, total: false },
        XColumn { title: "Importo".into(), width: 16.0, total: false },
    ];
    let mut data = Vec::new();
    for q in &a.quadri {
        for r in &q.righi {
            for (t, v) in &r.values {
                data.push(vec![XValue::Text(q.code.clone()), XValue::Text(r.code.clone()), XValue::Text(r.label.clone()), XValue::Text(t.clone()), XValue::Money(f(*v))]);
            }
        }
    }
    for w in &a.warnings {
        data.push(vec![XValue::Text("Avviso".into()), XValue::Empty, XValue::Text(w.clone()), XValue::Empty, XValue::Empty]);
    }
    export_table(path, "Dichiarazione IVA", &format!("Prospetto dichiarazione IVA - anno {}", a.year), &cols, &data)
}
