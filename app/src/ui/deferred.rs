//! Fatturazione differita: i DDT non ancora fatturati di un periodo, di un cliente o di tutti
//! (fine mese), diventano una fattura per cliente e condizione di pagamento, con i
//! riferimenti ai DDT (TD24 e DatiDDT nella fattura elettronica).

use std::collections::BTreeSet;

use chrono::{Datelike, NaiveDate};
use eframe::egui::{self, RichText};
use openfatt_core::model::{DocKind, DocStatus};
use openfatt_store::{DeferredRun, DocFilter, PendingDdt};
use rust_decimal::Decimal;

use super::theme::{self, icon, Tone};
use super::widgets::{self, eur, GRAY, RED};
use super::{Ctx, Nav};

pub struct Window {
    from: NaiveDate,
    to: NaiveDate,
    /// Data delle fatture.
    date: NaiveDate,
    /// Solo questo cliente (None = tutti).
    party: Option<i64>,
    issue: bool,
    list: Vec<PendingDdt>,
    checked: BTreeSet<i64>,
    /// I DDT vanno riletti (periodo cambiato o fatture appena create).
    stale: bool,
    /// DDT da spuntare alla prima lettura (scelti nell'elenco); None = quelli proposti.
    preset: Option<BTreeSet<i64>>,
    result: Option<DeferredRun>,
    /// Data dell'ultima fattura emessa (registro vendite, senza serie): le differite non
    /// possono avere una data anteriore se si emettono subito.
    last_issued: Option<NaiveDate>,
}

fn last_day(y: i32, m: u32) -> NaiveDate {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    NaiveDate::from_ymd_opt(ny, nm, 1).unwrap().pred_opt().unwrap()
}

impl Window {
    /// Periodo proposto: fino al 15 il mese precedente (la differita si emette entro il 15
    /// del mese dopo le consegne), poi il mese in corso fino a oggi.
    pub fn new(party: Option<i64>) -> Self {
        let today = chrono::Local::now().date_naive();
        let (from, to) = if today.day() <= 15 {
            let first = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap();
            let prev_last = first.pred_opt().unwrap();
            (NaiveDate::from_ymd_opt(prev_last.year(), prev_last.month(), 1).unwrap(), prev_last)
        } else {
            (NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap(), today)
        };
        Window { from, to, date: to, party, issue: false, list: Vec::new(), checked: BTreeSet::new(), stale: true, preset: None, result: None, last_issued: None }
    }

    /// Con i DDT scelti nell'elenco (anche di clienti diversi): periodo che li comprende.
    pub fn with_selection(store: &openfatt_store::Store, ids: &[i64]) -> Self {
        let mut w = Window::new(None);
        let dates: Vec<NaiveDate> = ids.iter().filter_map(|id| store.document(*id).ok()).map(|d| d.date).collect();
        if let (Some(min), Some(max)) = (dates.iter().min(), dates.iter().max()) {
            w.from = NaiveDate::from_ymd_opt(min.year(), min.month(), 1).unwrap();
            let today = chrono::Local::now().date_naive();
            w.to = last_day(max.year(), max.month()).min(today).max(*max);
            w.date = w.to;
        }
        w.preset = Some(ids.iter().copied().collect());
        w
    }

    fn reload(&mut self, cx: &mut Ctx) {
        self.stale = false;
        match cx.store.pending_delivery_notes(Some(self.from), self.to, None) {
            Ok(l) => self.list = l,
            Err(e) => {
                cx.msgs.err(e.to_string());
                self.list.clear();
            }
        }
        let sales = DocFilter {
            kinds: vec![DocKind::Invoice, DocKind::AccompanyingInvoice, DocKind::CreditNote, DocKind::DebitNote, DocKind::DownPaymentInvoice, DocKind::Fee],
            status: Some(DocStatus::Issued),
            ..Default::default()
        };
        self.last_issued = cx.store.documents(&sales).unwrap_or_default().into_iter().find(|d| d.series.trim().is_empty()).map(|d| d.date);
        self.checked = match self.preset.take() {
            Some(p) => self.list.iter().filter(|d| p.contains(&d.id)).map(|d| d.id).collect(),
            None => self.list.iter().filter(|d| d.suggested).map(|d| d.id).collect(),
        };
        if let Some(p) = self.party {
            if !self.list.iter().any(|d| d.party_id == p) {
                self.party = None;
            }
        }
    }

    /// DDT visibili (filtro sul cliente).
    fn visible(&self) -> impl Iterator<Item = &PendingDdt> {
        self.list.iter().filter(move |d| self.party.map(|p| d.party_id == p).unwrap_or(true))
    }

    /// Restituisce false quando la finestra va chiusa.
    pub fn show(&mut self, ctx: &egui::Context, cx: &mut Ctx) -> bool {
        if self.stale {
            self.reload(cx);
        }
        let mut open = true;
        let mut close = false;
        egui::Window::new("Fatturazione differita dei DDT")
            .open(&mut open)
            .collapsible(false)
            .default_width(820.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(theme::muted("I DDT emessi e non ancora fatturati (anche in parte) diventano una fattura per cliente e condizione di pagamento, con i riferimenti ai DDT. La fattura differita va emessa entro il 15 del mese successivo alle consegne."));
                ui.add_space(6.0);
                // ---- periodo e cliente ----
                ui.horizontal(|ui| {
                    ui.label("DDT dal");
                    if widgets::date_field(ui, "diff_from", &mut self.from) {
                        self.stale = true;
                    }
                    ui.label("al");
                    if widgets::date_field(ui, "diff_to", &mut self.to) {
                        self.stale = true;
                        self.date = self.to;
                    }
                    let today = chrono::Local::now().date_naive();
                    let first = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap();
                    let prev = first.pred_opt().unwrap();
                    if ui.small_button("Mese scorso").clicked() {
                        self.from = NaiveDate::from_ymd_opt(prev.year(), prev.month(), 1).unwrap();
                        self.to = prev;
                        self.date = prev;
                        self.stale = true;
                    }
                    if ui.small_button("Questo mese").clicked() {
                        self.from = first;
                        self.to = today;
                        self.date = today;
                        self.stale = true;
                    }
                    ui.add_space(12.0);
                    ui.label("Cliente");
                    let mut names: Vec<(i64, String)> = Vec::new();
                    for d in &self.list {
                        if !names.iter().any(|(id, _)| *id == d.party_id) {
                            names.push((d.party_id, d.party_name.clone()));
                        }
                    }
                    let current = self.party.and_then(|p| names.iter().find(|(id, _)| *id == p)).map(|(_, n)| n.clone()).unwrap_or_else(|| format!("Tutti ({})", names.len()));
                    egui::ComboBox::from_id_salt("diff_party").width(240.0).height(360.0).selected_text(widgets::truncate(&current, 34)).show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.party, None, format!("Tutti i clienti ({})", names.len()));
                        for (id, n) in &names {
                            ui.selectable_value(&mut self.party, Some(*id), n);
                        }
                    });
                });
                ui.add_space(4.0);
                theme::rule(ui);

                // ---- elenco per cliente ----
                let visible: Vec<PendingDdt> = self.visible().cloned().collect();
                if visible.is_empty() {
                    egui::Frame::new().inner_margin(18).show(ui, |ui| {
                        ui.label(theme::muted("Nessun DDT da fatturare nel periodo."));
                    });
                } else {
                    ui.horizontal(|ui| {
                        if ui.small_button("Tutti").clicked() {
                            self.checked.extend(visible.iter().map(|d| d.id));
                        }
                        if ui.small_button("Nessuno").clicked() {
                            for d in &visible {
                                self.checked.remove(&d.id);
                            }
                        }
                        if ui.small_button("Solo i proposti").on_hover_text("Esclude conto visione, riparazione, reso e simili (dalla causale del trasporto)").clicked() {
                            for d in &visible {
                                if d.suggested {
                                    self.checked.insert(d.id);
                                } else {
                                    self.checked.remove(&d.id);
                                }
                            }
                        }
                    });
                    egui::ScrollArea::vertical().id_salt("diff_list").max_height(380.0).auto_shrink([false, true]).show(ui, |ui| {
                        let mut i = 0;
                        while i < visible.len() {
                            let party = visible[i].party_id;
                            let mut j = i;
                            while j < visible.len() && visible[j].party_id == party {
                                j += 1;
                            }
                            let group = &visible[i..j];
                            let all_on = group.iter().all(|d| self.checked.contains(&d.id));
                            let n_on = group.iter().filter(|d| self.checked.contains(&d.id)).count();
                            let sum: Decimal = group.iter().filter(|d| self.checked.contains(&d.id)).map(|d| d.amount).sum();
                            let mut pays: Vec<Option<i64>> = group.iter().filter(|d| self.checked.contains(&d.id)).map(|d| d.payment_id).collect();
                            pays.dedup();
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                let mut on = all_on;
                                if ui.checkbox(&mut on, "").changed() {
                                    for d in group {
                                        if on {
                                            self.checked.insert(d.id);
                                        } else {
                                            self.checked.remove(&d.id);
                                        }
                                    }
                                }
                                ui.label(theme::medium(&group[0].party_name));
                                ui.label(theme::muted(format!("{} DDT su {}", n_on, group.len())));
                                if pays.len() > 1 {
                                    theme::pill(ui, &format!("{} fatture: pagamenti diversi", pays.len()), Tone::Warning);
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(theme::medium(format!("€ {}", eur(sum))));
                                });
                            });
                            for d in group {
                                ui.horizontal(|ui| {
                                    ui.add_space(22.0);
                                    let mut on = self.checked.contains(&d.id);
                                    if ui.checkbox(&mut on, format!("DDT {} del {}", d.number_label, d.date.format("%d/%m/%Y"))).changed() {
                                        if on {
                                            self.checked.insert(d.id);
                                        } else {
                                            self.checked.remove(&d.id);
                                        }
                                    }
                                    if !d.reason.is_empty() {
                                        ui.label(RichText::new(widgets::truncate(&d.reason, 30)).color(if d.suggested { GRAY } else { RED }));
                                    }
                                    if !d.payment_name.is_empty() {
                                        ui.label(RichText::new(widgets::truncate(&d.payment_name, 30)).color(GRAY).small());
                                    }
                                    if d.partial {
                                        theme::pill(ui, "fatturato in parte", Tone::Warning).on_hover_text("Si fattura solo quello che resta");
                                    }
                                    if ui.small_button(icon::ARROW_SQUARE_OUT).on_hover_text("Apri il DDT").clicked() {
                                        cx.nav.push(Nav::OpenDocument(d.id));
                                        close = true;
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(format!("€ {}", eur(d.amount)));
                                    });
                                });
                            }
                            ui.separator();
                            i = j;
                        }
                    });
                }

                // ---- risultato dell'ultima fatturazione ----
                if let Some(r) = &self.result {
                    ui.add_space(4.0);
                    egui::Frame::new().fill(theme::RAISED).corner_radius(6).inner_margin(8).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(theme::medium(format!("{} fatture create{}", r.created.len(), if r.issued > 0 { format!(", {} emesse", r.issued) } else { " in bozza".into() })));
                        egui::ScrollArea::vertical().id_salt("diff_done").max_height(110.0).show(ui, |ui| {
                            for d in &r.created {
                                ui.horizontal(|ui| {
                                    ui.label(format!("{} · {}", d.title(), widgets::truncate(&d.party.display_name(), 40)));
                                    if ui.small_button("Apri").clicked() {
                                        cx.nav.push(Nav::OpenDocument(d.id));
                                        close = true;
                                    }
                                });
                            }
                            for e in &r.errors {
                                ui.label(RichText::new(e).color(RED));
                            }
                        });
                    });
                }

                // ---- comandi ----
                ui.add_space(6.0);
                let chosen: Vec<&PendingDdt> = self.list.iter().filter(|d| self.checked.contains(&d.id)).collect();
                let mut groups: Vec<(i64, Option<i64>)> = chosen.iter().map(|d| (d.party_id, d.payment_id)).collect();
                groups.sort();
                groups.dedup();
                let total: Decimal = chosen.iter().map(|d| d.amount).sum();
                let last = chosen.iter().map(|d| d.date).max();
                ui.horizontal(|ui| {
                    ui.label("Data delle fatture");
                    widgets::date_field(ui, "diff_date", &mut self.date);
                    ui.checkbox(&mut self.issue, "Emetti subito").on_hover_text("Assegna il numero e conferma le fatture; altrimenti restano in bozza da controllare");
                    if let Some(l) = last {
                        if l > self.date {
                            ui.label(RichText::new(format!("la data non può essere anteriore all'ultimo DDT ({})", l.format("%d/%m/%Y"))).color(RED).small());
                        }
                    }
                    if let Some(l) = self.last_issued.filter(|l| *l > self.date && l.year() == self.date.year()) {
                        let t = format!("c'è già una fattura emessa il {}: con questa data la numerazione non sarebbe cronologica", l.format("%d/%m/%Y"));
                        ui.label(RichText::new(t).color(if self.issue { RED } else { GRAY }).small());
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let label = format!("Crea {} fattur{} da {} DDT", groups.len(), if groups.len() == 1 { "a" } else { "e" }, chosen.len());
                    let ok = !chosen.is_empty() && last.map(|l| l <= self.date).unwrap_or(false);
                    if theme::primary_enabled(ui, ok, icon::FILE_TEXT, &label).clicked() {
                        let ids: Vec<i64> = chosen.iter().map(|d| d.id).collect();
                        match cx.store.deferred_invoices(&ids, self.date, self.issue) {
                            Ok(r) => {
                                if r.errors.is_empty() {
                                    cx.msgs.ok(format!("{} fatture differite create", r.created.len()));
                                } else {
                                    cx.msgs.warn(format!("{} fatture create, {} clienti non fatturati", r.created.len(), r.errors.len()));
                                }
                                self.result = Some(r);
                                self.stale = true;
                            }
                            Err(e) => cx.msgs.err(e.to_string()),
                        }
                    }
                    ui.label(theme::muted(format!("Imponibile € {}", eur(total))));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::button(ui, icon::X, "Chiudi").clicked() {
                            close = true;
                        }
                    });
                });
            });
        open && !close
    }
}
