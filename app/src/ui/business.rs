//! Schermate aggiunte con le funzioni avanzate: documenti ricorrenti, agenti e provvigioni,
//! statistiche, agenda, carte fedeltà e buoni, negozio online, operazioni di fine anno,
//! conservazione e ordini NSO, utenti.

use eframe::egui::{self, RichText};

use super::grid::{Col, Grid, Val};
use super::theme::{self, icon, Tone};
use super::widgets::{eur, GRAY};
use super::{Ctx, Nav};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extra {
    Recurring,
    Agents,
    Stats,
    Agenda,
    Cards,
    Shop,
    YearEnd,
    Conservation,
}

#[derive(Default)]
pub struct State {
    pub agents: super::agents::State,
    pub stats: super::stats::State,
    pub agenda: super::agenda::State,
    pub cards: super::cards::State,
    pub shop: super::shop::State,
    pub yearend: super::yearend::State,
    pub conservation: super::conservation::State,
    recurring_confirm: Option<i64>,
    recurring_grid: Grid,
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, x: Extra) {
    match x {
        Extra::Recurring => recurring(ui, cx, st),
        Extra::Agents => super::agents::show(ui, cx, &mut st.agents),
        Extra::Stats => super::stats::show(ui, cx, &mut st.stats),
        Extra::Agenda => super::agenda::show(ui, cx, &mut st.agenda),
        Extra::Cards => super::cards::show(ui, cx, &mut st.cards),
        Extra::Shop => super::shop::show(ui, cx, &mut st.shop),
        Extra::YearEnd => super::yearend::show(ui, cx, &mut st.yearend),
        Extra::Conservation => super::conservation::show(ui, cx, &mut st.conservation),
    }
}

fn recurring(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let today = chrono::Local::now().date_naive();
    let list = cx.store.recurrences().unwrap_or_default();
    let due = list.iter().filter(|r| r.active && r.next_date <= today && r.end_date.map(|e| r.next_date <= e).unwrap_or(true)).count();
    theme::title_row(ui, &["Vendite"], "Documenti ricorrenti", None, |ui| {
        if theme::primary_enabled(ui, due > 0, icon::REPEAT, &format!("Genera i documenti scaduti ({})", due)).clicked() {
            if let Some(ids) = cx.msgs.check(cx.store.generate_recurring(today), "") {
                cx.msgs.ok(format!("{} documenti generati", ids.len()));
            }
        }
    });
    ui.label(RichText::new("Per rendere ricorrente un documento: aprirlo in bozza e scegliere Altro > Rendi ricorrente. Il documento resta come modello e le copie seguono la data prevista; nelle descrizioni {mese}, {anno} e {periodo} diventano il periodo del documento.").color(GRAY));
    ui.add_space(8.0);
    if list.is_empty() {
        ui.label(RichText::new("Nessun documento ricorrente.").color(GRAY));
        return;
    }
    type Row = (openfatt_store::Recurrence, Option<openfatt_core::model::Document>);
    let rows: Vec<Row> = list.iter().map(|r| (r.clone(), cx.store.document(r.template_id).ok())).collect();
    let open_req = std::cell::Cell::new(None);
    let del_req = std::cell::Cell::new(None);
    let (open_ref, del_ref) = (&open_req, &del_req);
    let state = move |r: &openfatt_store::Recurrence| if !r.active { "Terminato" } else if r.next_date <= today { "Da generare" } else if r.issue { "Emissione automatica" } else { "Genera bozze" };
    let cols: Vec<Col<Row>> = vec![
        Col::text("desc", "Descrizione", 240.0, |r: &Row| r.0.description.clone()),
        Col::text("kind", "Tipo", 120.0, |r: &Row| r.1.as_ref().map(|d| d.kind.label().to_string()).unwrap_or_default()).group(),
        Col::text("party", "Soggetto", 220.0, |r: &Row| r.1.as_ref().map(|d| d.party.display_name()).unwrap_or_else(|| "(eliminato)".into())).group().opens(|r: &Row| r.1.as_ref().and_then(|d| d.party_id).map(super::quick::Ref::Party)),
        Col::text("every", "Ogni", 100.0, |r: &Row| super::linetools::every_label(r.0.every_months)).group(),
        Col::new("next", "Prossimo", 90.0, |r: &Row| Val::Date(Some(r.0.next_date))),
        Col::new("end", "Fino al", 90.0, |r: &Row| Val::Date(r.0.end_date)),
        Col::new("generated", "Generati", 70.0, |r: &Row| Val::Num(rust_decimal::Decimal::from(r.0.generated as i64), 0)).right(),
        Col::text("state", "Stato", 150.0, move |r: &Row| state(&r.0).to_string()).group().render(move |ui: &mut egui::Ui, r: &Row| {
            let tone = if !r.0.active { Tone::Neutral } else if r.0.next_date <= today { Tone::Warning } else { Tone::Positive };
            theme::pill(ui, state(&r.0), tone);
        }),
        Col::text("act", "", 130.0, |_: &Row| String::new()).render(move |ui: &mut egui::Ui, r: &Row| {
            if ui.small_button("Apri modello").clicked() {
                open_ref.set(Some(r.0.template_id));
            }
            if ui.small_button(icon::TRASH).on_hover_text("Elimina la ricorrenza (il modello resta)").clicked() {
                del_ref.set(Some(r.0.id));
            }
        }),
    ];
    st.recurring_grid.name = "ricorrenti";
    ui.horizontal(|ui| st.recurring_grid.toolbar(ui, cx.store, cx.msgs, &cols, &rows, "Documenti ricorrenti"));
    let h = ui.available_height() - 4.0;
    st.recurring_grid.version = 0;
    st.recurring_grid.show(ui, cx.store, &cols, &rows, |r| r.0.id, h, false, 26.0);
    if let Some(d) = open_req.get() {
        cx.nav.push(Nav::OpenDocument(d));
    }
    if let Some(id) = del_req.get() {
        st.recurring_confirm = Some(id);
    }
    if let Some(id) = st.recurring_confirm {
        match super::widgets::confirm(ui.ctx(), "Elimina ricorrenza", "Eliminare la ricorrenza? Il documento modello e quelli già generati restano.") {
            Some(true) => {
                cx.msgs.check(cx.store.delete_recurrence(id), "Ricorrenza eliminata");
                st.recurring_confirm = None;
            }
            Some(false) => st.recurring_confirm = None,
            None => {}
        }
    }
    let _ = eur;
}
