//! Statistiche di vendita e acquisto, confronto con l'anno precedente e flussi di cassa previsti.

use chrono::{Datelike, NaiveDate};
use eframe::egui::{self, RichText};
use openfatt_core::money::fmt_it_trim;
use openfatt_store::{FlowRow, StatBy, StatRow};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use super::grid::{Col, Grid, Val};
use super::quick::Ref;
use super::theme::{self, icon};
use super::widgets::{self, eur, GRAY, RED};
use super::Ctx;

pub struct State {
    by: StatBy,
    from: NaiveDate,
    to: NaiveDate,
    year: i32,
    cache: Option<(u64, StatBy, NaiveDate, NaiveDate, Vec<StatRow>)>,
    months: Option<(u64, i32, Vec<(u32, Decimal, Decimal)>)>,
    flows: Option<(u64, Vec<FlowRow>)>,
    tab: u8,
    grid: Grid,
    codes: super::Cache<(), std::collections::HashMap<String, i64>>,
}

impl Default for State {
    fn default() -> Self {
        let today = chrono::Local::now().date_naive();
        State { by: StatBy::Customer, from: NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap(), to: today, year: today.year(), cache: None, months: None, flows: None, tab: 0, grid: Grid::new("statistiche"), codes: Default::default() }
    }
}

fn by_label(b: StatBy) -> &'static str {
    match b {
        StatBy::Month => "Mese",
        StatBy::Customer => "Cliente",
        StatBy::Product => "Articolo",
        StatBy::Category => "Categoria",
        StatBy::Agent => "Agente",
        StatBy::Supplier => "Fornitore (acquisti)",
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let gen = cx.store.generation();
    theme::title_row(ui, &["Analisi"], "Statistiche e flussi", None, |_| {});
    ui.horizontal(|ui| {
        for (i, t) in ["Andamento mensile", "Vendite per voce", "Flussi di cassa previsti"].iter().enumerate() {
            if ui.selectable_label(st.tab == i as u8, *t).clicked() {
                st.tab = i as u8;
            }
        }
    });
    ui.add_space(8.0);
    match st.tab {
        0 => monthly(ui, cx, st, gen),
        1 => breakdown(ui, cx, st, gen),
        _ => flows(ui, cx, st, gen),
    }
}

fn monthly(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, gen: u64) {
    ui.horizontal(|ui| {
        ui.label("Anno");
        ui.add(egui::DragValue::new(&mut st.year).range(2000..=2100));
    });
    if st.months.as_ref().map(|(g, y, _)| *g != gen || *y != st.year).unwrap_or(true) {
        st.months = Some((gen, st.year, cx.store.sales_by_month(st.year).unwrap_or_default()));
    }
    let data = st.months.as_ref().map(|(_, _, d)| d.clone()).unwrap_or_default();
    let labels: Vec<String> = ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"].iter().map(|s| s.to_string()).collect();
    let cur: Vec<f64> = data.iter().map(|d| d.1.to_f64().unwrap_or(0.0)).collect();
    let tot: Decimal = data.iter().map(|d| d.1).sum();
    let prev: Decimal = data.iter().map(|d| d.2).sum();
    theme::card(ui, &format!("Venduto {} (imponibile, al netto delle note di credito)", st.year), |ui| {
        ui.label(RichText::new(format!("€ {} · anno precedente € {}", eur(tot), eur(prev))).color(GRAY));
    }, |ui| {
        theme::bar_chart(ui, &labels, &cur, 220.0, |v| format!("€ {}", openfatt_core::money::fmt_it(Decimal::from_f64_retain(v).unwrap_or_default(), 0)));
    });
    ui.add_space(8.0);
    egui::ScrollArea::vertical().id_salt("months_scroll").show(ui, |ui| egui::Grid::new("months").striped(true).num_columns(4).spacing([24.0, 4.0]).show(ui, |ui| {
        for h in ["Mese", &st.year.to_string(), &(st.year - 1).to_string(), "Variazione"] {
            ui.label(RichText::new(h).strong());
        }
        ui.end_row();
        for (m, a, b) in &data {
            ui.label(&labels[(*m - 1) as usize]);
            ui.label(eur(*a));
            ui.label(RichText::new(eur(*b)).color(GRAY));
            if b.is_zero() {
                ui.label("-");
            } else {
                let v = (*a - *b) / *b * Decimal::ONE_HUNDRED;
                ui.label(RichText::new(format!("{}{}%", if v > Decimal::ZERO { "+" } else { "" }, fmt_it_trim(v, 0, 1))).color(if v < Decimal::ZERO { RED } else { theme::POSITIVE }));
            }
            ui.end_row();
        }
    }));
}

fn breakdown(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, gen: u64) {
    ui.horizontal(|ui| {
        ui.label("Per");
        egui::ComboBox::from_id_salt("stat_by").selected_text(by_label(st.by)).show_ui(ui, |ui| {
            for b in [StatBy::Customer, StatBy::Product, StatBy::Category, StatBy::Agent, StatBy::Month, StatBy::Supplier] {
                ui.selectable_value(&mut st.by, b, by_label(b));
            }
        });
        ui.label("dal");
        widgets::date_field(ui, "st_from", &mut st.from);
        ui.label("al");
        widgets::date_field(ui, "st_to", &mut st.to);
    });
    let stale = st.cache.as_ref().map(|(g, b, f, t, _)| *g != gen || *b != st.by || *f != st.from || *t != st.to).unwrap_or(true);
    if stale {
        st.cache = Some((gen, st.by, st.from, st.to, cx.store.statistics(st.by, st.from, st.to).unwrap_or_default()));
    }
    let rows = st.cache.as_ref().map(|c| c.4.clone()).unwrap_or_default();
    let total: Decimal = rows.iter().map(|r| r.revenue).sum();
    let margin: Decimal = rows.iter().map(|r| r.margin()).sum();
    ui.horizontal(|ui| {
        ui.label(theme::bold(format!("Totale € {}", eur(total))).size(18.0));
        if st.by != StatBy::Supplier {
            ui.label(RichText::new(format!("· margine stimato € {}", eur(margin))).color(GRAY));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::button(ui, icon::FILE_XLS, "Excel").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Excel", &["xlsx"]).set_file_name("statistica.xlsx").save_file() {
                    let title = format!("Statistica per {} dal {} al {}", by_label(st.by).to_lowercase(), st.from.format("%d/%m/%Y"), st.to.format("%d/%m/%Y"));
                    cx.msgs.check(crate::reports::stats_xlsx(&p, &title, &rows), "Esportazione completata");
                }
            }
        });
    });
    ui.add_space(4.0);
    let top: Vec<&StatRow> = rows.iter().take(12).collect();
    if !top.is_empty() && st.by != StatBy::Month {
        let labels: Vec<String> = top.iter().map(|r| widgets::truncate(&r.label, 14)).collect();
        let vals: Vec<f64> = top.iter().map(|r| r.revenue.to_f64().unwrap_or(0.0)).collect();
        theme::bar_chart(ui, &labels, &vals, 160.0, |v| format!("€ {}", openfatt_core::money::fmt_it(Decimal::from_f64_retain(v).unwrap_or_default(), 0)));
    }
    let by = st.by;
    let codes_rc = st.codes.get(cx.store.generation(), &(), || cx.store.products("", true).unwrap_or_default().into_iter().map(|p| (p.code, p.id)).collect());
    let codes: &std::collections::HashMap<String, i64> = &codes_rc;
    let cols: Vec<Col<StatRow>> = vec![
        Col::text("label", by_label(by), 320.0, |r: &StatRow| r.label.clone()).opens(move |r: &StatRow| match by {
            StatBy::Customer | StatBy::Supplier => r.key.parse().ok().map(Ref::Party),
            StatBy::Product => codes.get(&r.key).copied().map(Ref::Product),
            _ => None,
        }),
        Col::new("qty", "Quantità", 90.0, |r: &StatRow| Val::Num(r.qty, 2)).right().total(),
        Col::money("revenue", if by == StatBy::Supplier { "Acquistato" } else { "Venduto" }, 110.0, |r: &StatRow| r.revenue).total(),
        Col::money("cost", "Costo", 100.0, |r: &StatRow| r.cost).total(),
        Col::money("margin", "Margine", 100.0, |r: &StatRow| r.margin()).total(),
        Col::new("margin_pct", "Margine %", 80.0, |r: &StatRow| r.margin_pct().map(|m| Val::Num(m, 1)).unwrap_or(Val::Text(String::new()))).right(),
        Col::new("docs", "Documenti", 80.0, |r: &StatRow| Val::Num(Decimal::from(r.documents as i64), 0)).right(),
    ];
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            st.grid.toolbar(ui, cx.store, cx.msgs, &cols, &rows, "Statistiche");
        });
    });
    let h = ui.available_height() - 4.0;
    st.grid.version = 0;
    st.grid.show(ui, cx.store, &cols, &rows, |r| {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        r.key.hash(&mut h);
        (h.finish() >> 1) as i64
    }, h, false, 22.0);
}

fn flows(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, gen: u64) {
    let today = chrono::Local::now().date_naive();
    if st.flows.as_ref().map(|(g, _)| *g != gen).unwrap_or(true) {
        st.flows = Some((gen, cx.store.cash_flow(today, 12).unwrap_or_default()));
    }
    let rows = st.flows.as_ref().map(|(_, r)| r.clone()).unwrap_or_default();
    let balance: Decimal = cx.store.account_balances().unwrap_or_default().iter().map(|b| b.balance).sum();
    ui.label(RichText::new(format!("Incassi e pagamenti attesi dallo scadenzario, a partire dal saldo attuale di cassa e banche (€ {}). Le scadenze già passate e non saldate sono nel primo mese come arretrati.", eur(balance))).color(GRAY));
    ui.add_space(6.0);
    let labels: Vec<String> = rows.iter().map(|r| r.month.clone()).collect();
    let mut running = balance;
    let mut series = Vec::new();
    for r in &rows {
        running += r.incoming + r.overdue_in - r.outgoing - r.overdue_out;
        series.push(running.to_f64().unwrap_or(0.0));
    }
    theme::card(ui, "Saldo previsto a fine mese", |_| {}, |ui| {
        theme::bar_chart(ui, &labels, &series, 200.0, |v| format!("€ {}", openfatt_core::money::fmt_it(Decimal::from_f64_retain(v).unwrap_or_default(), 0)));
    });
    ui.add_space(6.0);
    egui::ScrollArea::vertical().id_salt("flows_scroll").show(ui, |ui| egui::Grid::new("flows").striped(true).num_columns(6).spacing([24.0, 4.0]).show(ui, |ui| {
        for h in ["Mese", "Da incassare", "Da pagare", "Arretrati attivi", "Arretrati passivi", "Saldo previsto"] {
            ui.label(RichText::new(h).strong());
        }
        ui.end_row();
        let mut running = balance;
        for r in &rows {
            running += r.incoming + r.overdue_in - r.outgoing - r.overdue_out;
            ui.label(&r.month);
            ui.label(eur(r.incoming));
            ui.label(eur(r.outgoing));
            ui.label(RichText::new(eur(r.overdue_in)).color(if r.overdue_in.is_zero() { GRAY } else { RED }));
            ui.label(RichText::new(eur(r.overdue_out)).color(if r.overdue_out.is_zero() { GRAY } else { RED }));
            ui.label(RichText::new(eur(running)).color(if running < Decimal::ZERO { RED } else { theme::INK }));
            ui.end_row();
        }
    }));
}
