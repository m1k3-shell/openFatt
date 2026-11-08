//! Schermate aggiunte con le funzioni avanzate: documenti ricorrenti, agenti e provvigioni,
//! statistiche, agenda, carte fedeltà e buoni, negozio online, operazioni di fine anno,
//! conservazione e ordini NSO, utenti.

use eframe::egui::{self, RichText};

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
    egui::Grid::new("recurring").striped(true).num_columns(8).spacing([14.0, 6.0]).show(ui, |ui| {
        for h in ["Descrizione", "Modello", "Ogni", "Prossimo", "Fino al", "Generati", "Stato", ""] {
            ui.label(RichText::new(h).strong());
        }
        ui.end_row();
        for r in &list {
            let t = cx.store.document(r.template_id).ok();
            ui.label(&r.description);
            ui.label(t.as_ref().map(|d| format!("{} - {}", d.kind.label(), d.party.display_name())).unwrap_or_else(|| "(eliminato)".into()));
            ui.label(super::linetools::every_label(r.every_months));
            ui.label(r.next_date.format("%d/%m/%Y").to_string());
            ui.label(r.end_date.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_else(|| "-".into()));
            ui.label(format!("{}{}", r.generated, r.last_generated.map(|d| format!(" (ultimo {})", d.format("%d/%m/%Y"))).unwrap_or_default()));
            if !r.active {
                theme::pill(ui, "Terminato", Tone::Neutral);
            } else if r.next_date <= today {
                theme::pill(ui, "Da generare", Tone::Warning);
            } else {
                theme::pill(ui, if r.issue { "Emissione automatica" } else { "Genera bozze" }, Tone::Positive);
            }
            ui.horizontal(|ui| {
                if ui.small_button("Apri modello").clicked() {
                    cx.nav.push(Nav::OpenDocument(r.template_id));
                }
                if ui.small_button(icon::TRASH).on_hover_text("Elimina la ricorrenza (il modello resta)").clicked() {
                    st.recurring_confirm = Some(r.id);
                }
            });
            ui.end_row();
        }
    });
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
