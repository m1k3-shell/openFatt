//! Agenti e provvigioni: anagrafica, provvigioni maturate, insoluti e liquidazioni.

use chrono::{Datelike, NaiveDate};
use eframe::egui::{self, RichText};
use openfatt_core::money::fmt_it_trim;
use openfatt_store::{Agent, CommissionRow};
use rust_decimal::Decimal;

use super::theme::{self, icon, Tone};
use super::widgets::{self, eur, GRAY, RED};
use super::{Ctx, Nav};

pub struct State {
    selected: Option<i64>,
    editing: Option<Agent>,
    from: NaiveDate,
    to: NaiveDate,
    rows: Option<(i64, Vec<CommissionRow>)>,
    confirm_settle: bool,
    cat: (String, Decimal),
}

impl Default for State {
    fn default() -> Self {
        let today = chrono::Local::now().date_naive();
        State { selected: None, editing: None, from: NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap(), to: today, rows: None, confirm_settle: false, cat: (String::new(), Decimal::ZERO) }
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let today = chrono::Local::now().date_naive();
    theme::title_row(ui, &["Analisi"], "Agenti e provvigioni", None, |ui| {
        if theme::primary(ui, icon::PLUS, "Nuovo agente").clicked() {
            st.editing = Some(Agent::default());
        }
    });
    edit_window(ui, cx, st);
    let agents = cx.store.agents(true).unwrap_or_default();
    if agents.is_empty() {
        ui.label(RichText::new("Nessun agente. L'agente si indica nella scheda del cliente (passa da solo nei documenti) o direttamente nel documento.").color(GRAY));
        return;
    }
    if st.selected.is_none() {
        st.selected = agents.iter().find(|a| a.active).map(|a| a.id);
    }
    ui.horizontal_top(|ui| {
        theme::card_frame().show(ui, |ui| ui.vertical(|ui| {
            ui.set_width(260.0);
            ui.label(theme::medium("Agenti"));
            for a in &agents {
                let sel = st.selected == Some(a.id);
                let label = format!("{}  ·  {}%{}", a.name, fmt_it_trim(a.rate, 0, 2), if a.active { "" } else { "  (non attivo)" });
                if ui.selectable_label(sel, label).clicked() {
                    st.selected = Some(a.id);
                    st.rows = None;
                }
            }
        }));
        ui.add_space(12.0);
        let Some(agent) = st.selected.and_then(|id| agents.iter().find(|a| a.id == id)).cloned() else {
            ui.label(RichText::new("Scegliere un agente a sinistra.").color(GRAY));
            return;
        };
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(theme::bold(&agent.name).size(20.0));
                theme::pill(ui, if agent.on_collected { "Sull'incassato" } else { "Sul fatturato" }, Tone::Neutral);
                if theme::button(ui, icon::PENCIL_SIMPLE, "Modifica").clicked() {
                    st.editing = Some(agent.clone());
                }
            });
            ui.horizontal(|ui| {
                ui.label("Dal");
                let a = widgets::date_field(ui, "ag_from", &mut st.from);
                ui.label("al");
                let b = widgets::date_field(ui, "ag_to", &mut st.to);
                if a || b {
                    st.rows = None;
                }
            });
            if st.rows.as_ref().map(|(id, _)| *id != agent.id).unwrap_or(true) {
                st.rows = Some((agent.id, cx.store.commissions(agent.id, st.from, st.to, today).unwrap_or_default()));
            }
            let rows = st.rows.as_ref().map(|(_, r)| r.clone()).unwrap_or_default();
            let tot = |f: fn(&CommissionRow) -> Decimal| -> Decimal { rows.iter().map(f).sum() };
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (l, v) in [("Imponibile", tot(|r| r.base)), ("Provvigioni", tot(|r| r.commission)), ("Maturate", tot(|r| r.matured)), ("Già liquidate", tot(|r| r.settled)), ("Da liquidare", tot(|r| r.to_settle())), ("Insoluti dei clienti", tot(|r| r.overdue))] {
                    theme::card_frame().show(ui, |ui| ui.vertical(|ui| {
                        ui.vertical(|ui| {
                            ui.label(theme::muted(l).size(12.0));
                            ui.label(theme::bold(format!("€ {}", eur(v))).size(18.0).color(if l.starts_with("Insoluti") && v > Decimal::ZERO { RED } else { theme::INK }));
                        });
                    }));
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let due = tot(|r| r.to_settle());
                if theme::primary_enabled(ui, !due.is_zero(), icon::CHECK, &format!("Liquida € {}", eur(due))).clicked() {
                    st.confirm_settle = true;
                }
                if theme::button(ui, icon::FILE_PDF, "Estratto conto PDF").clicked() {
                    match crate::reports::commissions_pdf(cx.store, &agent, st.from, st.to, &rows) {
                        Ok(pdf) => {
                            if let Err(e) = crate::services::open_bytes(&format!("provvigioni_{}.pdf", agent.name.replace(' ', "_")), &pdf) {
                                cx.msgs.err(e);
                            }
                        }
                        Err(e) => cx.msgs.err(e),
                    }
                }
                if theme::button(ui, icon::FILE_XLS, "Excel").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Excel", &["xlsx"]).set_file_name(format!("provvigioni_{}.xlsx", agent.name.replace(' ', "_"))).save_file() {
                        cx.msgs.check(crate::reports::commissions_xlsx(&p, &agent, &rows), "Esportazione completata");
                    }
                }
            });
            egui::ScrollArea::vertical().id_salt("comm").max_height(ui.available_height() - 140.0).show(ui, |ui| {
                egui::Grid::new("comm_rows").striped(true).num_columns(9).spacing([12.0, 4.0]).show(ui, |ui| {
                    for h in ["Documento", "Cliente", "Imponibile", "Provvigione", "Incassato", "Maturata", "Liquidata", "Insoluti", ""] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for r in &rows {
                        ui.label(&r.title);
                        ui.label(widgets::truncate(&r.party, 28));
                        ui.label(eur(r.base));
                        ui.label(eur(r.commission));
                        ui.label(format!("{}%", fmt_it_trim(r.collected * Decimal::ONE_HUNDRED, 0, 0)));
                        ui.label(eur(r.matured));
                        ui.label(eur(r.settled));
                        ui.label(RichText::new(if r.overdue.is_zero() { "-".to_string() } else { eur(r.overdue) }).color(if r.overdue.is_zero() { GRAY } else { RED }));
                        if ui.small_button("Apri").clicked() {
                            cx.nav.push(Nav::OpenDocument(r.document_id));
                        }
                        ui.end_row();
                    }
                });
                let settl = cx.store.settlements(agent.id).unwrap_or_default();
                if !settl.is_empty() {
                    ui.add_space(10.0);
                    ui.label(theme::medium("Liquidazioni registrate"));
                    for s in settl.iter().rev() {
                        ui.horizontal(|ui| {
                            ui.label(format!("{} · periodo {} - {} · € {} · {} documenti", &s.created_at[..10.min(s.created_at.len())], s.date_from.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default(), s.date_to.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default(), eur(s.total), s.rows.len()));
                            if ui.small_button(icon::TRASH).on_hover_text("Annulla la liquidazione").clicked() {
                                cx.msgs.check(cx.store.delete_settlement(s.id), "Liquidazione annullata");
                                st.rows = None;
                            }
                        });
                    }
                }
            });
            if st.confirm_settle {
                match widgets::confirm(ui.ctx(), "Liquidazione provvigioni", "Registrare come liquidate le provvigioni maturate del periodo?") {
                    Some(true) => {
                        if let Some(s) = cx.msgs.check(cx.store.settle_commissions(agent.id, st.from, st.to, today), "") {
                            cx.msgs.ok(format!("Liquidate provvigioni per € {}", eur(s.total)));
                        }
                        st.confirm_settle = false;
                        st.rows = None;
                    }
                    Some(false) => st.confirm_settle = false,
                    None => {}
                }
            }
        });
    });
}

fn edit_window(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let Some(a) = &mut st.editing else { return };
    let mut close = false;
    egui::Window::new(if a.id == 0 { "Nuovo agente" } else { "Modifica agente" }).collapsible(false).default_width(520.0).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
        egui::Grid::new("agent_form").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Nome");
            ui.add(egui::TextEdit::singleline(&mut a.name).desired_width(280.0));
            ui.end_row();
            ui.label("Email");
            ui.add(egui::TextEdit::singleline(&mut a.email).desired_width(280.0));
            ui.end_row();
            ui.label("Telefono");
            ui.add(egui::TextEdit::singleline(&mut a.phone).desired_width(160.0));
            ui.end_row();
            ui.label("P.IVA / Cod. fiscale");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut a.vat_number).desired_width(120.0));
                ui.add(egui::TextEdit::singleline(&mut a.tax_code).desired_width(160.0));
            });
            ui.end_row();
            ui.label("Provvigione %");
            widgets::dec_field(ui, "ag_rate", &mut a.rate, 70.0, 2);
            ui.end_row();
            ui.label("Maturazione");
            ui.horizontal(|ui| {
                ui.radio_value(&mut a.on_collected, true, "sull'incassato");
                ui.radio_value(&mut a.on_collected, false, "sul fatturato");
            });
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut a.active, "Attivo");
            ui.end_row();
        });
        ui.label(RichText::new("Provvigioni per categoria di articoli (sostituiscono quella predefinita; la provvigione indicata nell'articolo o nella riga ha la precedenza)").small().color(GRAY));
        let mut remove = None;
        for (i, (c, r)) in a.category_rates.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{}: {}%", c, fmt_it_trim(*r, 0, 2)));
                if ui.small_button(icon::TRASH).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            a.category_rates.remove(i);
        }
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut st.cat.0).desired_width(160.0).hint_text("categoria"));
            widgets::dec_field(ui, "ag_cat_rate", &mut st.cat.1, 60.0, 2);
            if ui.button("Aggiungi").clicked() && !st.cat.0.trim().is_empty() {
                a.category_rates.push((st.cat.0.trim().to_string(), st.cat.1));
                st.cat = (String::new(), Decimal::ZERO);
            }
        });
        ui.label("Note");
        ui.add(egui::TextEdit::multiline(&mut a.notes).desired_rows(2).desired_width(f32::INFINITY));
        ui.separator();
        ui.horizontal(|ui| {
            if theme::primary(ui, icon::CHECK, "Salva").clicked() {
                if let Some(saved) = cx.msgs.check(cx.store.save_agent(a), "Agente salvato") {
                    st.selected = Some(saved.id);
                    st.rows = None;
                    close = true;
                }
            }
            if a.id != 0 && theme::button(ui, icon::TRASH, "Elimina").clicked() && cx.msgs.check(cx.store.delete_agent(a.id), "Agente eliminato").is_some() {
                st.selected = None;
                close = true;
            }
            if theme::button(ui, icon::X, "Annulla").clicked() {
                close = true;
            }
        });
    });
    if close {
        st.editing = None;
    }
}
