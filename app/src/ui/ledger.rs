//! Scadenzario (incassi e pagamenti) e prima nota.

use chrono::{Datelike, NaiveDate};
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use super::grid::{Col, Grid, Val};
use super::quick::{self, Ref};
use openfatt_store::DueRow;
use openfatt_core::codes;
use openfatt_core::model::CashEntry;
use rust_decimal::Decimal;

use std::collections::BTreeSet;

use openfatt_core::mailtext::{self, MailTemplates};
use openfatt_store::ReminderCandidate;

use super::theme::{self, icon, Tone};
use super::widgets::{self, eur, GRAY, GREEN, ORANGE, RED};
use super::{Ctx, Nav};
use crate::mailer;

#[derive(PartialEq, Clone, Copy, Default)]
enum DuesView {
    #[default]
    Dues,
    Reminders,
}

/// Voci del menu Pagamenti.
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum PayView {
    All,
    In,
    Out,
    Until,
}

#[derive(PartialEq, Clone, Copy)]
enum DueFilter {
    Receivable,
    Payable,
    All,
}

pub struct State {
    view: DuesView,
    rem_sel: BTreeSet<String>,
    rem_init: bool,
    attach_pdfs: bool,
    due_filter: DueFilter,
    only_open: bool,
    until: Option<NaiveDate>,
    search: String,
    paying: Option<(i64, NaiveDate, Decimal, String, bool, String)>,
    confirm_unpay: Option<i64>,
    cash_from: NaiveDate,
    cash_to: NaiveDate,
    cash_account: Option<String>,
    new_entry: Option<(CashEntry, bool)>,
    due_grid: Grid,
    cash_grid: Grid,
}

impl Default for State {
    fn default() -> Self {
        let today = chrono::Local::now().date_naive();
        State {
            view: DuesView::Dues,
            rem_sel: BTreeSet::new(),
            rem_init: false,
            attach_pdfs: true,
            due_filter: DueFilter::Receivable,
            only_open: true,
            until: None,
            search: String::new(),
            paying: None,
            confirm_unpay: None,
            cash_from: NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap(),
            cash_to: NaiveDate::from_ymd_opt(today.year(), 12, 31).unwrap(),
            cash_account: None,
            new_entry: None,
            due_grid: Grid::new("scadenzario"),
            cash_grid: Grid::new("prima-nota"),
        }
    }
}

impl State {
    /// Apre lo scadenzario sulla vista scelta dal menu Pagamenti.
    pub fn show_view(&mut self, v: PayView) {
        self.view = DuesView::Dues;
        self.until = None;
        match v {
            PayView::All => {
                self.due_filter = DueFilter::All;
                self.only_open = false;
            }
            PayView::In => {
                self.due_filter = DueFilter::Receivable;
                self.only_open = false;
            }
            PayView::Out => {
                self.due_filter = DueFilter::Payable;
                self.only_open = false;
            }
            PayView::Until => {
                self.due_filter = DueFilter::All;
                self.only_open = true;
                self.until = Some(chrono::Local::now().date_naive());
            }
        }
    }

    /// Apre lo scadenzario sulla scheda dei solleciti, con la selezione proposta.
    pub fn show_reminders(&mut self) {
        self.view = DuesView::Reminders;
        self.rem_init = false;
    }
}

fn rem_key(c: &ReminderCandidate) -> String {
    match c.party_id {
        Some(id) => id.to_string(),
        None => format!("n:{}", c.party_name),
    }
}

pub fn show_dues(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let title = if st.view == DuesView::Reminders {
        "Invio solleciti"
    } else if st.until.is_some() {
        "Da saldare"
    } else {
        match st.due_filter {
            DueFilter::All => "Pagamenti",
            DueFilter::Receivable => "Pagamenti: entrate",
            DueFilter::Payable => "Pagamenti: uscite",
        }
    };
    super::title(ui, title);
    let today = chrono::Local::now().date_naive();
    let tpl = mailer::templates(cx.store);
    let candidates = cx.store.reminder_candidates(today, tpl.grace_days, tpl.min_days_between).unwrap_or_default();
    if st.view == DuesView::Reminders {
        reminders(ui, cx, st, &candidates, &tpl);
        return;
    }
    let accounts = cx.store.accounts().unwrap_or_default();

    // registrazione incasso/pagamento
    let mut close = false;
    if let Some((id, date, amount, account, writeoff, label)) = &mut st.paying {
        egui::Window::new("Registra incasso / pagamento").collapsible(false).show(ui.ctx(), |ui| {
            ui.label(label.as_str());
            egui::Grid::new("pay").num_columns(2).show(ui, |ui| {
                ui.label("Data");
                widgets::date_field(ui, "pay_date", date);
                ui.end_row();
                ui.label("Importo");
                widgets::dec_field(ui, "pay_amount", amount, 100.0, 2);
                ui.end_row();
                ui.label("Conto");
                egui::ComboBox::from_id_salt("pay_acc").selected_text(account.clone()).show_ui(ui, |ui| {
                    for a in &accounts {
                        ui.selectable_value(account, a.clone(), a);
                    }
                });
                ui.end_row();
                ui.label("");
                ui.checkbox(writeoff, "Chiudi la scadenza con abbuono della differenza");
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if ui.button("Registra").clicked() {
                    if cx.msgs.check(cx.store.pay_due(*id, *date, *amount, account, *writeoff), "Movimento registrato in prima nota").is_some() {
                        close = true;
                    }
                }
                if ui.button("Annulla").clicked() {
                    close = true;
                }
            });
        });
    }
    if close {
        st.paying = None;
    }
    if let Some(id) = st.confirm_unpay {
        match widgets::confirm(ui.ctx(), "Annulla pagamenti", "Annullare tutti i pagamenti registrati su questa scadenza (e i movimenti di prima nota collegati)?") {
            Some(true) => {
                cx.msgs.check(cx.store.unpay_due(id), "Pagamenti annullati");
                st.confirm_unpay = None;
            }
            Some(false) => st.confirm_unpay = None,
            None => {}
        }
    }

    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut st.search).hint_text(format!("{}  Cliente, fornitore, documento", icon::MAGNIFYING_GLASS)).desired_width(240.0));
        ui.selectable_value(&mut st.due_filter, DueFilter::All, "Tutti");
        ui.selectable_value(&mut st.due_filter, DueFilter::Receivable, "Entrate");
        ui.selectable_value(&mut st.due_filter, DueFilter::Payable, "Uscite");
        ui.separator();
        ui.checkbox(&mut st.only_open, "solo da saldare");
        ui.separator();
        ui.label("Fino al");
        widgets::opt_date_field(ui, "due_until", &mut st.until);
    });
    let list: Vec<_> = cx
        .store
        .due_list_with_drafts(st.only_open, st.until)
        .unwrap_or_default()
        .into_iter()
        .filter(|d| match st.due_filter {
            DueFilter::Receivable => d.receivable,
            DueFilter::Payable => !d.receivable,
            DueFilter::All => true,
        })
        .filter(|d| {
            let q = st.search.trim().to_lowercase();
            q.is_empty() || d.party_name.to_lowercase().contains(&q) || d.doc_label.to_lowercase().contains(&q)
        })
        .collect();
    #[derive(Clone, Copy)]
    enum DueAction {
        Open(i64),
        Pay(i64),
        Unpay(i64),
    }
    let action = std::cell::Cell::new(None);
    let act = &action;
    let color = move |d: &DueRow| if d.residual() <= Decimal::ZERO { GREEN } else if d.date < today { RED } else if d.date <= today + chrono::Duration::days(7) { ORANGE } else { ui_text() };
    let cols: Vec<Col<DueRow>> = vec![
        Col::new("date", "Scadenza", 90.0, |d: &DueRow| Val::Date(Some(d.date))).render(move |ui: &mut egui::Ui, d: &DueRow| {
            ui.label(RichText::new(d.date.format("%d/%m/%Y").to_string()).color(color(d)));
        }),
        Col::text("doc", "Documento", 240.0, |d: &DueRow| d.doc_label.clone()).render(move |ui: &mut egui::Ui, d: &DueRow| {
            if ui.link(&d.doc_label).clicked() {
                act.set(Some(DueAction::Open(d.document_id)));
            }
            if d.draft {
                theme::pill(ui, "bozza", Tone::Warning);
            }
        }),
        Col::text("party", "Soggetto", 260.0, |d: &DueRow| d.party_name.clone()).group().opens(|d: &DueRow| d.party_id.map(Ref::Party)),
        Col::text("dir", "Tipo", 80.0, |d: &DueRow| if d.receivable { "Entrata".to_string() } else { "Uscita".to_string() }).hidden().group(),
        Col::text("kind", "Documento di", 140.0, |d: &DueRow| d.kind.label().to_string()).hidden().group(),
        Col::text("mp", "Modalità", 140.0, |d: &DueRow| codes::describe(codes::MODALITA_PAGAMENTO, &d.mp_code).to_string()).group().render(|ui: &mut egui::Ui, d: &DueRow| {
            ui.label(codes::describe(codes::MODALITA_PAGAMENTO, &d.mp_code));
            if d.batch_id.is_some() {
                theme::pill(ui, "in distinta", Tone::Neutral).on_hover_text("Presentata in banca con una distinta (Pagamenti > Distinte emesse)");
            }
        }),
        Col::money("amount", "Importo", 95.0, |d: &DueRow| d.amount).total(),
        Col::money("paid", "Pagato", 95.0, |d: &DueRow| d.paid).hidden().total(),
        Col::money("res", "Residuo", 95.0, |d: &DueRow| d.residual()).total().render(move |ui: &mut egui::Ui, d: &DueRow| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(eur(d.residual())).color(color(d)));
                if !d.writeoff.is_zero() {
                    ui.label(RichText::new(format!("(abbuono {})", eur(d.writeoff))).small().color(GRAY));
                }
            });
        }),
        Col::new("late", "Giorni di ritardo", 90.0, move |d: &DueRow| Val::Num(Decimal::from((today - d.date).num_days().max(0)), 0)).hidden().right(),
        Col::text("act", "", 170.0, |_: &DueRow| String::new()).render(move |ui: &mut egui::Ui, d: &DueRow| {
            if d.residual() > Decimal::ZERO && ui.small_button(if d.receivable { "Incassa" } else { "Paga" }).clicked() {
                act.set(Some(DueAction::Pay(d.id)));
            }
            if (!d.paid.is_zero() || !d.writeoff.is_zero()) && ui.small_button("Annulla pag.").clicked() {
                act.set(Some(DueAction::Unpay(d.id)));
            }
        }),
    ];
    ui.horizontal(|ui| {
        let visible: Vec<&DueRow> = list.iter().filter(|d| st.due_grid.accepts(&cols, d)).collect();
        let overdue: Decimal = visible.iter().filter(|d| d.date < today).map(|d| d.residual()).sum();
        let total: Decimal = visible.iter().map(|d| d.residual()).sum();
        ui.label(RichText::new(format!("Residuo totale € {}   ·   di cui scaduto € {}", eur(total), eur(overdue))).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            st.due_grid.toolbar(ui, cx.store, cx.msgs, &cols, &list, "Scadenzario");
        });
    });
    let avail = ui.available_height() - 4.0;
    st.due_grid.version = 0;
    let ev = st.due_grid.show(ui, cx.store, &cols, &list, |d| d.id, avail, false, 24.0);
    if let Some(d) = ev.opened.and_then(|id| list.iter().find(|d| d.id == id)) {
        cx.nav.push(Nav::OpenDocument(d.document_id));
    }
    match action.get() {
        Some(DueAction::Open(id)) => cx.nav.push(Nav::OpenDocument(id)),
        Some(DueAction::Pay(id)) => {
            if let Some(d) = list.iter().find(|d| d.id == id) {
                st.paying = Some((d.id, today, d.residual(), "Banca".into(), false, format!("{} - {}", d.doc_label, d.party_name)));
            }
        }
        Some(DueAction::Unpay(id)) => st.confirm_unpay = Some(id),
        None => {}
    }
}

/// Clienti con pagamenti scaduti: scelta e preparazione dei solleciti.
fn reminders(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, list: &[ReminderCandidate], tpl: &MailTemplates) {
    if !st.rem_init {
        // proposta: chi ha un indirizzo e non è stato sollecitato da poco
        st.rem_sel = list.iter().filter(|c| (!c.email.is_empty() || !c.pec.is_empty()) && !c.recently_reminded).map(rem_key).collect();
        st.rem_init = true;
    }
    st.rem_sel.retain(|k| list.iter().any(|c| &rem_key(c) == k));
    let total: Decimal = list.iter().map(|c| c.overdue_total).sum();
    ui.label(theme::muted(format!(
        "Clienti con pagamenti scaduti da più di {} giorni: {} · totale scaduto € {}. Il livello sale a ogni sollecito: promemoria, secondo sollecito, ultimo avviso (via PEC se il cliente ne ha una).",
        tpl.grace_days,
        list.len(),
        eur(total)
    )));
    ui.add_space(6.0);
    if list.is_empty() {
        theme::card(ui, "Nessun sollecito da inviare", |_| {}, |ui| {
            ui.label(theme::muted("Tutte le scadenze dei clienti sono incassate o entro la tolleranza impostata (Opzioni > Email e solleciti)."));
        });
        return;
    }
    let selected: Vec<&ReminderCandidate> = list.iter().filter(|c| st.rem_sel.contains(&rem_key(c))).collect();
    ui.horizontal(|ui| {
        let n = selected.len();
        if theme::primary_enabled(ui, n > 0, icon::PAPER_PLANE_TILT, &format!("Prepara {} {}", n, if n == 1 { "sollecito" } else { "solleciti" })).clicked() {
            let msgs: Vec<_> = selected.iter().map(|c| mailer::for_reminder(cx.store, c, c.next_level, st.attach_pdfs)).collect();
            cx.nav.push(Nav::Mail(msgs));
        }
        ui.checkbox(&mut st.attach_pdfs, "allega i PDF delle fatture scadute");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::ghost(ui, true, icon::GEAR, "Testi e tolleranza").clicked() {
                cx.nav.push(Nav::MailSettings);
            }
            if theme::ghost(ui, true, icon::SQUARE, "Nessuno").clicked() {
                st.rem_sel.clear();
            }
            if theme::ghost(ui, true, icon::CHECK_SQUARE, "Tutti").clicked() {
                st.rem_sel = list.iter().map(rem_key).collect();
            }
        });
    });
    ui.add_space(6.0);
    let avail = ui.available_height();
    let today = chrono::Local::now().date_naive();
    let mut toggle = None;
    let mut single = None;
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        TableBuilder::new(ui)
            .striped(false)
            .resizable(false)
            .max_scroll_height((avail - 40.0).max(120.0))
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(34.0))
            .column(Column::remainder().at_least(180.0))
            .column(Column::initial(240.0))
            .column(Column::initial(80.0))
            .column(Column::initial(120.0))
            .column(Column::initial(80.0))
            .column(Column::initial(200.0))
            .column(Column::initial(150.0))
            .column(Column::initial(120.0))
            .header(30.0, |mut h| {
                for t in ["", "Cliente", "Indirizzo", "Scadenze", "Scaduto", "Ritardo", "Ultimo sollecito", "Prossimo", ""] {
                    h.col(|ui| {
                        ui.label(theme::medium(t).size(13.0).color(theme::MUTED));
                    });
                }
            })
            .body(|body| {
                body.rows(44.0, list.len(), |mut row| {
                    let c = &list[row.index()];
                    let key = rem_key(c);
                    let mut on = st.rem_sel.contains(&key);
                    row.set_selected(on);
                    row.col(|ui| {
                        if ui.checkbox(&mut on, "").changed() {
                            toggle = Some(key.clone());
                        }
                    });
                    row.col(|ui| {
                        match c.party_id {
                            Some(id) => {
                                quick::link(ui, c.party_name.clone(), Ref::Party(id));
                            }
                            None => {
                                ui.label(theme::medium(&c.party_name));
                            }
                        }
                    });
                    row.col(|ui| {
                        if !c.email.is_empty() {
                            ui.label(&c.email);
                        } else if !c.pec.is_empty() {
                            ui.label(format!("PEC {}", c.pec));
                        } else {
                            ui.label(RichText::new("nessun indirizzo").color(theme::NEGATIVE));
                        }
                    });
                    row.col(|ui| {
                        let tip: Vec<String> = c.dues.iter().map(|d| format!("{} - scad. {} - € {}", d.doc_label, d.date.format("%d/%m/%Y"), eur(d.residual()))).collect();
                        ui.label(c.dues.len().to_string()).on_hover_text(tip.join("\n"));
                    });
                    row.col(|ui| {
                        ui.label(theme::medium(format!("€ {}", eur(c.overdue_total))));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(format!("{} gg", c.days_late)).color(if c.days_late > 60 { theme::NEGATIVE } else { theme::INK }));
                    });
                    row.col(|ui| match c.last_reminder {
                        Some((ts, lvl)) => {
                            let days = (today - ts.date()).num_days();
                            ui.label(RichText::new(format!("{} · {}", ts.format("%d/%m/%Y"), mailtext::reminder_level_label(lvl).to_lowercase())).color(if c.recently_reminded { theme::WARNING } else { theme::INK }))
                                .on_hover_text(if c.recently_reminded { format!("{} giorni fa: ancora presto per un nuovo sollecito", days) } else { format!("{} giorni fa", days) });
                        }
                        None => {
                            ui.label(theme::muted("mai"));
                        }
                    });
                    row.col(|ui| {
                        let tone = match c.next_level {
                            1 => Tone::Neutral,
                            2 => Tone::Warning,
                            _ => Tone::Negative,
                        };
                        theme::pill(ui, mailtext::reminder_level_label(c.next_level), tone);
                    });
                    row.col(|ui| {
                        if theme::ghost(ui, true, icon::ENVELOPE_SIMPLE, "Sollecita").clicked() {
                            single = Some(key.clone());
                        }
                    });
                    if row.response().clicked() {
                        toggle = Some(key);
                    }
                });
            });
    });
    if let Some(k) = toggle {
        if !st.rem_sel.remove(&k) {
            st.rem_sel.insert(k);
        }
    }
    if let Some(k) = single {
        if let Some(c) = list.iter().find(|c| rem_key(c) == k) {
            cx.nav.push(Nav::Mail(vec![mailer::for_reminder(cx.store, c, c.next_level, st.attach_pdfs)]));
        }
    }
}

fn ui_text() -> egui::Color32 {
    egui::Color32::from_rgb(40, 40, 40)
}

pub fn show_cash(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    super::title(ui, "Prima nota");
    let accounts = cx.store.accounts().unwrap_or_default();

    let mut close = false;
    if let Some((e, is_out)) = &mut st.new_entry {
        egui::Window::new("Nuovo movimento").collapsible(false).show(ui.ctx(), |ui| {
            egui::Grid::new("cash_new").num_columns(2).show(ui, |ui| {
                ui.label("Data");
                widgets::date_field(ui, "ce_date", &mut e.date);
                ui.end_row();
                ui.label("Tipo");
                ui.horizontal(|ui| {
                    ui.radio_value(is_out, false, "Entrata");
                    ui.radio_value(is_out, true, "Uscita");
                });
                ui.end_row();
                ui.label("Descrizione");
                ui.add(egui::TextEdit::singleline(&mut e.description).desired_width(300.0));
                ui.end_row();
                ui.label("Importo");
                widgets::dec_field(ui, "ce_amount", &mut e.amount, 100.0, 2);
                ui.end_row();
                ui.label("Conto");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("ce_acc").selected_text(e.account.clone()).show_ui(ui, |ui| {
                        for a in &accounts {
                            ui.selectable_value(&mut e.account, a.clone(), a);
                        }
                    });
                    ui.add(egui::TextEdit::singleline(&mut e.account).desired_width(120.0).hint_text("o nuovo conto"));
                });
                ui.end_row();
                ui.label("Categoria");
                ui.add(egui::TextEdit::singleline(&mut e.category).desired_width(200.0).hint_text("es. Affitto, Utenze, Giroconto"));
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if ui.button("Registra").clicked() {
                    let mut entry = e.clone();
                    entry.amount = if *is_out { -entry.amount.abs() } else { entry.amount.abs() };
                    if cx.msgs.check(cx.store.add_cash_entry(&entry), "Movimento registrato").is_some() {
                        close = true;
                    }
                }
                if ui.button("Annulla").clicked() {
                    close = true;
                }
            });
        });
    }
    if close {
        st.new_entry = None;
    }

    ui.horizontal(|ui| {
        if ui.button("\u{E3D4} Movimento").clicked() {
            st.new_entry = Some((CashEntry::default(), false));
        }
        ui.label("Dal");
        widgets::date_field(ui, "cash_from", &mut st.cash_from);
        ui.label("al");
        widgets::date_field(ui, "cash_to", &mut st.cash_to);
        egui::ComboBox::from_id_salt("cash_acc_f").selected_text(st.cash_account.clone().unwrap_or_else(|| "Tutti i conti".into())).show_ui(ui, |ui| {
            ui.selectable_value(&mut st.cash_account, None, "Tutti i conti");
            for a in &accounts {
                ui.selectable_value(&mut st.cash_account, Some(a.clone()), a);
            }
        });
    });
    let balances = cx.store.account_balances().unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        for b in &balances {
            ui.label(RichText::new(format!("{}: € {}", b.account, eur(b.balance))).strong().color(if b.balance.is_sign_negative() { RED } else { GREEN }));
            ui.separator();
        }
    });
    let list = cx.store.cash_entries(Some(st.cash_from), Some(st.cash_to), st.cash_account.as_deref()).unwrap_or_default();
    let delete_req = std::cell::Cell::new(None);
    let open_req = std::cell::Cell::new(None);
    let (delete_ref, open_ref) = (&delete_req, &open_req);
    let cols: Vec<Col<CashEntry>> = vec![
        Col::new("date", "Data", 90.0, |e: &CashEntry| Val::Date(Some(e.date))),
        Col::text("desc", "Descrizione", 380.0, |e: &CashEntry| e.description.clone()).render(move |ui: &mut egui::Ui, e: &CashEntry| match e.document_id {
            Some(d) => {
                if ui.link(&e.description).clicked() {
                    open_ref.set(Some(d));
                }
            }
            None => {
                ui.label(&e.description);
            }
        }),
        Col::text("account", "Conto", 110.0, |e: &CashEntry| e.account.clone()).group(),
        Col::text("cat", "Categoria", 130.0, |e: &CashEntry| e.category.clone()).group(),
        Col::text("month", "Mese", 80.0, |e: &CashEntry| e.date.format("%Y-%m").to_string()).hidden().group(),
        Col::money("in", "Entrate", 100.0, |e: &CashEntry| e.amount.max(Decimal::ZERO)).total(),
        Col::money("out", "Uscite", 100.0, |e: &CashEntry| (-e.amount).max(Decimal::ZERO)).total(),
        Col::text("act", "", 34.0, |_: &CashEntry| String::new()).render(move |ui: &mut egui::Ui, e: &CashEntry| {
            if e.due_date_id.is_none() && ui.small_button("\u{E4A6}").clicked() {
                delete_ref.set(Some(e.id));
            }
        }),
    ];
    ui.horizontal(|ui| {
        let visible: Vec<&CashEntry> = list.iter().filter(|e| st.cash_grid.accepts(&cols, e)).collect();
        let ins: Decimal = visible.iter().filter(|e| e.amount > Decimal::ZERO).map(|e| e.amount).sum();
        let outs: Decimal = visible.iter().filter(|e| e.amount < Decimal::ZERO).map(|e| -e.amount).sum();
        ui.label(format!("Nel periodo: entrate € {} · uscite € {} · saldo € {}", eur(ins), eur(outs), eur(ins - outs)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            st.cash_grid.toolbar(ui, cx.store, cx.msgs, &cols, &list, "Prima nota");
        });
    });
    let avail = ui.available_height() - 4.0;
    st.cash_grid.version = 0;
    st.cash_grid.show(ui, cx.store, &cols, &list, |e| e.id, avail, false, 22.0);
    if let Some(d) = open_req.get() {
        cx.nav.push(Nav::OpenDocument(d));
    }
    let delete = delete_req.get();
    if let Some(id) = delete {
        cx.msgs.check(cx.store.delete_cash_entry(id), "Movimento eliminato");
    }
}
