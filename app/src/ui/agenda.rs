//! Agenda: calendario del mese con appuntamenti, attività e scadenze di pagamento.

use chrono::{Datelike, Duration, NaiveDate};
use eframe::egui::{self, Color32, RichText, Sense, Vec2};
use openfatt_store::{AgendaItem, AgendaKind};

use super::theme::{self, icon, Tone};
use super::widgets::{self, eur, GRAY};
use super::{Ctx, Nav};

pub struct State {
    month: NaiveDate,
    day: NaiveDate,
    editing: Option<AgendaItem>,
}

impl Default for State {
    fn default() -> Self {
        let t = chrono::Local::now().date_naive();
        State { month: NaiveDate::from_ymd_opt(t.year(), t.month(), 1).unwrap(), day: t, editing: None }
    }
}

const MESI: [&str; 12] = ["Gennaio", "Febbraio", "Marzo", "Aprile", "Maggio", "Giugno", "Luglio", "Agosto", "Settembre", "Ottobre", "Novembre", "Dicembre"];

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let today = chrono::Local::now().date_naive();
    theme::title_row(ui, &["Analisi"], "Agenda", None, |ui| {
        if theme::primary(ui, icon::PLUS, "Nuovo impegno").clicked() {
            st.editing = Some(AgendaItem { date: st.day, ..Default::default() });
        }
    });
    edit_window(ui, cx, st);
    let first = st.month;
    let next = openfatt_store::add_months(first, 1);
    let last = next.pred_opt().unwrap();
    let items = cx.store.agenda(first, last).unwrap_or_default();
    let dues = cx.store.due_list(true, Some(last)).unwrap_or_default();
    ui.horizontal_top(|ui| {
        // calendario
        theme::card_frame().show(ui, |ui| ui.vertical(|ui| {
            ui.set_width(560.0);
            ui.horizontal(|ui| {
                if theme::icon_button(ui, icon::CARET_LEFT, "Mese precedente").clicked() {
                    st.month = NaiveDate::from_ymd_opt(if first.month() == 1 { first.year() - 1 } else { first.year() }, if first.month() == 1 { 12 } else { first.month() - 1 }, 1).unwrap();
                }
                ui.label(theme::bold(format!("{} {}", MESI[first.month0() as usize], first.year())).size(18.0));
                if theme::icon_button(ui, icon::CARET_RIGHT, "Mese successivo").clicked() {
                    st.month = next;
                }
                if ui.button("Oggi").clicked() {
                    st.month = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap();
                    st.day = today;
                }
            });
            ui.add_space(4.0);
            let cell = Vec2::new(78.0, 64.0);
            egui::Grid::new("cal").spacing([2.0, 2.0]).show(ui, |ui| {
                for d in ["Lun", "Mar", "Mer", "Gio", "Ven", "Sab", "Dom"] {
                    ui.label(RichText::new(d).size(12.0).color(GRAY));
                }
                ui.end_row();
                let offset = first.weekday().num_days_from_monday() as i64;
                let start = first - Duration::days(offset);
                for w in 0..6 {
                    for i in 0..7 {
                        let d = start + Duration::days(w * 7 + i);
                        let (rect, resp) = ui.allocate_exact_size(cell, Sense::click());
                        let in_month = d.month() == first.month();
                        let fill = if d == st.day { theme::ACTIVE } else if d == today { Color32::from_rgb(0xe8, 0xf0, 0xfb) } else { theme::SURFACE };
                        ui.painter().rect_filled(rect, 4, fill);
                        ui.painter().text(rect.left_top() + Vec2::new(5.0, 3.0), egui::Align2::LEFT_TOP, d.day().to_string(), egui::FontId::proportional(13.0), if in_month { theme::INK } else { theme::CHART_MUTED });
                        let n = items.iter().filter(|a| a.date == d).count();
                        let nd = dues.iter().filter(|x| x.date == d).count();
                        let mut y = rect.top() + 22.0;
                        for a in items.iter().filter(|a| a.date == d).take(2) {
                            let c = if a.done { theme::CHART_MUTED } else { theme::ACCENT };
                            ui.painter().text(egui::pos2(rect.left() + 5.0, y), egui::Align2::LEFT_TOP, widgets::truncate(&a.title, 10), egui::FontId::proportional(11.0), c);
                            y += 13.0;
                        }
                        if n > 2 {
                            ui.painter().text(egui::pos2(rect.left() + 5.0, y), egui::Align2::LEFT_TOP, format!("+{}", n - 2), egui::FontId::proportional(11.0), GRAY);
                        }
                        if nd > 0 {
                            ui.painter().circle_filled(rect.left_top() + Vec2::new(30.0, 11.0), 4.0, theme::WARNING);
                        }
                        if resp.clicked() {
                            st.day = d;
                        }
                        if resp.double_clicked() {
                            st.editing = Some(AgendaItem { date: d, ..Default::default() });
                        }
                    }
                    ui.end_row();
                }
            });
            ui.label(RichText::new("Il punto arancione indica scadenze di pagamento. Doppio clic su un giorno per un nuovo impegno.").small().color(GRAY));
        }));
        ui.add_space(12.0);
        // giorno
        ui.vertical(|ui| {
            ui.label(theme::bold(format!("{} {} {}", ["Lunedì", "Martedì", "Mercoledì", "Giovedì", "Venerdì", "Sabato", "Domenica"][st.day.weekday().num_days_from_monday() as usize], st.day.day(), MESI[st.day.month0() as usize])).size(18.0));
            let day_items = cx.store.agenda(st.day, st.day).unwrap_or_default();
            if day_items.is_empty() {
                ui.label(RichText::new("Nessun impegno.").color(GRAY));
            }
            for a in &day_items {
                ui.horizontal(|ui| {
                    let mut done = a.done;
                    if ui.checkbox(&mut done, "").on_hover_text("Fatto").changed() {
                        let mut x = a.clone();
                        x.done = done;
                        cx.msgs.check(cx.store.save_agenda_item(&x), "");
                    }
                    ui.label(RichText::new(if a.time.is_empty() { "tutto il giorno".to_string() } else { a.time.clone() }).color(GRAY));
                    theme::pill(ui, a.kind.label(), Tone::Neutral);
                    let t = RichText::new(&a.title);
                    if ui.add(egui::Label::new(if a.done { t.strikethrough().color(GRAY) } else { t }).sense(Sense::click())).on_hover_text("Modifica").clicked() {
                        st.editing = Some(a.clone());
                    }
                    if let Some(pid) = a.party_id {
                        if let Ok(Some(p)) = cx.store.party(pid) {
                            ui.label(RichText::new(format!("· {}", p.display_name())).color(GRAY));
                        }
                    }
                });
                if !a.notes.is_empty() {
                    ui.label(RichText::new(&a.notes).small().color(GRAY));
                }
            }
            let day_dues: Vec<_> = dues.iter().filter(|d| d.date == st.day).collect();
            if !day_dues.is_empty() {
                ui.add_space(8.0);
                ui.label(theme::medium("Scadenze di pagamento"));
                for d in day_dues {
                    ui.horizontal(|ui| {
                        ui.label(if d.receivable { "da incassare" } else { "da pagare" });
                        ui.label(format!("€ {}", eur(d.residual())));
                        ui.label(RichText::new(format!("{} · {}", d.doc_label, d.party_name)).color(GRAY));
                        if ui.small_button("Apri").clicked() {
                            cx.nav.push(Nav::OpenDocument(d.document_id));
                        }
                    });
                }
            }
            let late = cx.store.agenda_overdue(today).unwrap_or_default();
            if !late.is_empty() {
                ui.add_space(8.0);
                ui.label(theme::medium(format!("Attività arretrate ({})", late.len())));
                for a in late.iter().take(10) {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(a.date.format("%d/%m").to_string()).color(theme::NEGATIVE));
                        if ui.link(&a.title).clicked() {
                            st.editing = Some(a.clone());
                        }
                    });
                }
            }
        });
    });
}

fn edit_window(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let Some(a) = &mut st.editing else { return };
    let mut close = false;
    egui::Window::new(if a.id == 0 { "Nuovo impegno" } else { "Impegno" }).collapsible(false).default_width(460.0).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
        egui::Grid::new("ag_item").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Tipo");
            ui.horizontal(|ui| {
                for k in AgendaKind::ALL {
                    ui.radio_value(&mut a.kind, k, k.label());
                }
            });
            ui.end_row();
            ui.label("Oggetto");
            ui.add(egui::TextEdit::singleline(&mut a.title).desired_width(300.0));
            ui.end_row();
            ui.label("Data e ora");
            ui.horizontal(|ui| {
                widgets::date_field(ui, "ag_date", &mut a.date);
                ui.add(egui::TextEdit::singleline(&mut a.time).desired_width(60.0).hint_text("09:30"));
            });
            ui.end_row();
            ui.label("Cliente / fornitore");
            let current = a.party_id.and_then(|id| cx.store.party(id).ok().flatten()).map(|p| p.display_name()).unwrap_or_default();
            ui.horizontal(|ui| {
                if let Some(p) = widgets::party_picker(ui, cx.store, "ag_party", &current, None, 240.0) {
                    a.party_id = Some(p.id);
                }
                if a.party_id.is_some() && ui.small_button(icon::X).clicked() {
                    a.party_id = None;
                }
            });
            ui.end_row();
            ui.label("Assegnato a");
            ui.add(egui::TextEdit::singleline(&mut a.assigned_to).desired_width(160.0).hint_text("utente (vuoto = tutti)"));
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut a.done, "Fatto");
            ui.end_row();
        });
        ui.label("Note");
        ui.add(egui::TextEdit::multiline(&mut a.notes).desired_rows(3).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            if theme::primary(ui, icon::CHECK, "Salva").clicked() && cx.msgs.check(cx.store.save_agenda_item(a), "Impegno salvato").is_some() {
                st.day = a.date;
                close = true;
            }
            if a.id != 0 && theme::button(ui, icon::TRASH, "Elimina").clicked() {
                cx.msgs.check(cx.store.delete_agenda_item(a.id), "Impegno eliminato");
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
