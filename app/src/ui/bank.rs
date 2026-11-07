//! Distinte bancarie: Ri.Ba. e addebiti SDD da presentare, bonifici da disporre,
//! distinte emesse con incassi, insoluti e annullo.

use std::collections::BTreeSet;

use chrono::NaiveDate;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use openfatt_core::fiscal;
use openfatt_core::model::{Party, PartyRole};
use openfatt_store::{BankBatch, BankKind, BankSettings, BatchStatus, DueRow};
use rust_decimal::Decimal;

use super::theme::{self, icon, Tone};
use super::widgets::{self, eur};
use super::{Ctx, Nav};
use crate::bankfiles;
use crate::services;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tab {
    #[default]
    Riba,
    Sdd,
    Sct,
    Batches,
}

impl Tab {
    fn kind(self) -> Option<BankKind> {
        match self {
            Tab::Riba => Some(BankKind::Riba),
            Tab::Sdd => Some(BankKind::Sdd),
            Tab::Sct => Some(BankKind::Sct),
            Tab::Batches => None,
        }
    }
    pub fn from_name(s: &str) -> Tab {
        match s {
            "sdd" => Tab::Sdd,
            "bonifici" | "sct" => Tab::Sct,
            "emesse" => Tab::Batches,
            _ => Tab::Riba,
        }
    }
}

pub struct State {
    tab: Tab,
    sel: BTreeSet<i64>,
    sel_for: Option<Tab>,
    until: Option<NaiveDate>,
    execution: NaiveDate,
    problems: Option<Vec<String>>,
    open_batch: Option<i64>,
    settle: Option<(i64, NaiveDate, String)>,
    confirm_cancel: Option<i64>,
    confirm_unpaid: Option<(i64, String)>,
}

impl Default for State {
    fn default() -> Self {
        let today = chrono::Local::now().date_naive();
        State {
            tab: Tab::Riba,
            sel: BTreeSet::new(),
            sel_for: None,
            until: None,
            execution: bankfiles::business_day(today.succ_opt().unwrap()),
            problems: None,
            open_batch: None,
            settle: None,
            confirm_cancel: None,
            confirm_unpaid: None,
        }
    }
}

impl State {
    pub fn on_tab(t: Tab) -> Self {
        State { tab: t, ..Default::default() }
    }
}

/// Problema che impedisce di presentare la scadenza (dati mancanti nell'anagrafica).
fn row_problem(kind: BankKind, p: Option<&Party>) -> Option<String> {
    let Some(p) = p else { return Some("anagrafica non collegata".into()) };
    let iban_ok = fiscal::iban_valido(&p.iban);
    match kind {
        BankKind::Riba => {
            let abi_cab = p.bank_abi.trim().len() == 5 && p.bank_cab.trim().len() == 5;
            if !(abi_cab || (iban_ok && p.iban.trim().to_uppercase().starts_with("IT"))) {
                return Some("manca la banca d'appoggio".into());
            }
            if p.vat_number.trim().is_empty() && p.tax_code.trim().is_empty() {
                return Some("manca codice fiscale / P.IVA".into());
            }
            None
        }
        BankKind::Sdd => {
            if !iban_ok {
                Some("manca l'IBAN".into())
            } else if p.sdd_mandate_id.trim().is_empty() {
                Some("manca il mandato SDD".into())
            } else {
                None
            }
        }
        BankKind::Sct => (!iban_ok).then(|| "manca l'IBAN".to_string()),
    }
}

fn bank_text(kind: BankKind, p: &Party) -> String {
    let iban = p.iban.trim().replace(' ', "");
    let short = if iban.len() > 12 { format!("{}…{}", &iban[..6], &iban[iban.len() - 4..]) } else { iban.clone() };
    match kind {
        BankKind::Riba if !p.bank_name.trim().is_empty() => format!("{} · {}", p.bank_name.trim(), if iban.is_empty() { format!("ABI {} CAB {}", p.bank_abi, p.bank_cab) } else { short }),
        BankKind::Sdd => format!("{} · mandato {}{}", short, p.sdd_mandate_id.trim(), if p.sdd_scheme == "B2B" { " (B2B)" } else { "" }),
        _ => short,
    }
}

fn company_problem(kind: BankKind, s: &BankSettings, iban: &str) -> Option<&'static str> {
    if openfatt_bank::it_bank_codes(iban).is_none() {
        return Some("Manca l'IBAN italiano dell'azienda (Impostazioni > Azienda).");
    }
    match kind {
        BankKind::Riba if s.sia.trim().len() != 5 => Some("Per le Ri.Ba. serve il codice SIA dell'azienda."),
        BankKind::Sdd if s.cuc.trim().is_empty() || !openfatt_bank::creditor_id_valid(&s.creditor_id) => Some("Per gli addebiti SDD servono il codice CUC e l'identificativo del creditore (CID)."),
        BankKind::Sct if s.cuc.trim().is_empty() => Some("Per i bonifici serve il codice CUC dell'azienda."),
        _ => None,
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let pending: Vec<(BankKind, Vec<DueRow>)> = BankKind::ALL.iter().map(|k| (*k, cx.store.bank_pending(*k).unwrap_or_default())).collect();
    let batches = cx.store.batches(None).unwrap_or_default();
    let count = |k: BankKind| pending.iter().find(|p| p.0 == k).map(|p| p.1.len()).unwrap_or(0);
    let sum = |k: BankKind| pending.iter().find(|p| p.0 == k).map(|p| p.1.iter().map(|d| d.residual()).sum::<Decimal>()).unwrap_or_default();
    let sub = theme::subline(&[
        (&format!("€ {} di Ri.Ba. e SDD da presentare", eur(sum(BankKind::Riba) + sum(BankKind::Sdd))), false),
        (" · ", false),
        (&format!("€ {} di bonifici da disporre", eur(sum(BankKind::Sct))), false),
    ]);
    theme::title_row(ui, &["Contabilità"], "Distinte bancarie", Some(sub), |ui| {
        if theme::ghost(ui, true, icon::GEAR, "Codici della banca").clicked() {
            cx.nav.push(Nav::BankSettings);
        }
    });
    ui.horizontal(|ui| {
        for (t, label, n) in [
            (Tab::Riba, "Ri.Ba. da presentare", count(BankKind::Riba)),
            (Tab::Sdd, "Addebiti SDD da presentare", count(BankKind::Sdd)),
            (Tab::Sct, "Bonifici da disporre", count(BankKind::Sct)),
            (Tab::Batches, "Distinte emesse", batches.iter().filter(|b| b.status == BatchStatus::Issued).count()),
        ] {
            ui.selectable_value(&mut st.tab, t, format!("{} ({})", label, n));
        }
    });
    ui.add_space(8.0);
    dialogs(ui, cx, st);
    match st.tab.kind() {
        Some(kind) => pending_view(ui, cx, st, kind, pending.into_iter().find(|p| p.0 == kind).map(|p| p.1).unwrap_or_default()),
        None => batches_view(ui, cx, st, &batches),
    }
}

fn dialogs(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let ctx = ui.ctx().clone();
    if let Some(list) = &st.problems {
        let mut close = false;
        egui::Window::new(format!("{}  Dati da completare", icon::WARNING)).collapsible(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).default_width(560.0).show(&ctx, |ui| {
            ui.label("Il file non è stato creato. Correggere:");
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for p in list {
                    ui.label(format!("•  {}", p));
                }
            });
            ui.add_space(8.0);
            if theme::primary(ui, icon::CHECK, "Ho capito").clicked() {
                close = true;
            }
        });
        if close {
            st.problems = None;
        }
    }
    let mut close = false;
    if let Some((id, date, account)) = &mut st.settle {
        let accounts = cx.store.accounts().unwrap_or_default();
        let b = cx.store.batches(None).unwrap_or_default().into_iter().find(|b| b.id == *id);
        let incoming = b.as_ref().map(|b| b.kind != BankKind::Sct).unwrap_or(true);
        egui::Window::new(if incoming { "Registra gli incassi della distinta" } else { "Registra i pagamenti della distinta" })
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(&ctx, |ui| {
                if let Some(b) = &b {
                    ui.label(theme::medium(format!("{} · {} voci aperte", b.label(), b.open_items)));
                }
                ui.label(theme::muted(if incoming { "Per le Ri.Ba. e gli SDD conviene registrare l'incasso alla data dell'accredito indicata dalla banca." } else { "Data in cui la banca ha eseguito i bonifici." }));
                egui::Grid::new("settle").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                    ui.label("Data");
                    widgets::date_field(ui, "settle_date", date);
                    ui.end_row();
                    ui.label("Conto");
                    egui::ComboBox::from_id_salt("settle_acc").selected_text(account.clone()).show_ui(ui, |ui| {
                        for a in &accounts {
                            ui.selectable_value(account, a.clone(), a);
                        }
                    });
                    ui.end_row();
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::primary(ui, icon::CHECK, "Registra").clicked() {
                        if let Some(n) = cx.msgs.check(cx.store.settle_batch(*id, *date, account), "") {
                            cx.msgs.ok(format!("{} {} registrati in prima nota", n, if incoming { "incassi" } else { "pagamenti" }));
                            close = true;
                        }
                    }
                    if theme::button(ui, icon::X, "Annulla").clicked() {
                        close = true;
                    }
                });
            });
    }
    if close {
        st.settle = None;
    }
    if let Some(id) = st.confirm_cancel {
        match widgets::confirm(&ctx, "Annulla la distinta", "Annullare la distinta? Le scadenze torneranno tra quelle da presentare. Farlo solo se il file non è stato caricato in banca o se la banca l'ha rifiutato.") {
            Some(true) => {
                cx.msgs.check(cx.store.cancel_batch(id), "Distinta annullata");
                st.confirm_cancel = None;
            }
            Some(false) => st.confirm_cancel = None,
            None => {}
        }
    }
    if let Some((item, label)) = st.confirm_unpaid.clone() {
        match widgets::confirm(&ctx, "Insoluto", &format!("Segnare come insoluta {}? Se l'incasso era registrato viene annullato, e la scadenza torna da incassare (si potrà ripresentare o sollecitare).", label)) {
            Some(true) => {
                cx.msgs.check(cx.store.mark_batch_item_unpaid(item, ""), "Scadenza segnata come insoluta");
                st.confirm_unpaid = None;
            }
            Some(false) => st.confirm_unpaid = None,
            None => {}
        }
    }
}

fn pending_view(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, kind: BankKind, all: Vec<DueRow>) {
    let settings = cx.store.bank_settings().unwrap_or_default();
    let company = cx.store.company().unwrap_or_default();
    let list: Vec<DueRow> = all.into_iter().filter(|d| st.until.is_none_or(|u| d.date <= u)).collect();
    let parties: Vec<Option<Party>> = list.iter().map(|d| d.party_id.and_then(|id| cx.store.party(id).ok().flatten())).collect();
    let problems: Vec<Option<String>> = parties.iter().map(|p| row_problem(kind, p.as_ref())).collect();
    let today = chrono::Local::now().date_naive();
    let past = |d: &DueRow| kind == BankKind::Riba && d.date <= today;
    if st.sel_for != Some(st.tab) {
        st.sel = list.iter().zip(&problems).filter(|(d, p)| p.is_none() && !past(d)).map(|(d, _)| d.id).collect();
        st.sel_for = Some(st.tab);
    }
    st.sel.retain(|id| list.iter().any(|d| d.id == *id));

    if let Some(msg) = company_problem(kind, &settings, &company.iban) {
        egui::Frame::new().fill(egui::Color32::from_rgb(0xff, 0xf4, 0xd6)).corner_radius(8.0).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}  {}", icon::WARNING, msg)).color(theme::WARNING));
                ui.label(theme::muted("I codici li comunica la banca insieme al servizio di home banking."));
                if theme::ghost(ui, true, icon::ARROW_RIGHT, "Inserisci i codici").clicked() {
                    cx.nav.push(Nav::BankSettings);
                }
            });
        });
        ui.add_space(8.0);
    }
    let selected: Vec<&DueRow> = list.iter().filter(|d| st.sel.contains(&d.id)).collect();
    let sel_total: Decimal = selected.iter().map(|d| d.residual()).sum();
    ui.horizontal(|ui| {
        let n = selected.len();
        let label = match kind {
            BankKind::Sct => format!("Crea distinta bonifici ({} · € {})", n, eur(sel_total)),
            _ => format!("Crea distinta ({} · € {})", n, eur(sel_total)),
        };
        if theme::primary_enabled(ui, n > 0, icon::FILE_ARROW_DOWN, &label).on_hover_text("Crea il file da caricare nell'home banking e registra la distinta").clicked() {
            let dues: Vec<DueRow> = selected.iter().map(|d| (*d).clone()).collect();
            create(cx, st, kind, &dues);
        }
        if kind == BankKind::Sct {
            ui.label(theme::muted("Esecuzione il"));
            widgets::date_field(ui, "sct_exec", &mut st.execution);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            widgets::opt_date_field(ui, "bank_until", &mut st.until);
            ui.label(theme::muted("Solo scadenze fino al"));
        });
    });
    ui.add_space(6.0);
    if list.is_empty() {
        theme::card(ui, "Niente da presentare", |_| {}, |ui| {
            ui.label(theme::muted(match kind {
                BankKind::Riba => "Nessuna scadenza aperta di clienti con pagamento Ri.Ba. (modalità MP12) che non sia già in una distinta.",
                BankKind::Sdd => "Nessuna scadenza aperta di clienti con pagamento SDD (MP19, MP20, MP21) che non sia già in una distinta. Per usare gli addebiti diretti: modalità di pagamento SDD e mandato firmato nell'anagrafica del cliente.",
                BankKind::Sct => "Nessuna scadenza aperta di fornitori con pagamento tramite bonifico (MP05) che non sia già in una distinta.",
            }));
        });
        return;
    }
    let mut toggle = None;
    let mut fix = None;
    let avail = ui.available_height();
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        TableBuilder::new(ui)
            .striped(false)
            .resizable(false)
            .sense(egui::Sense::click())
            .max_scroll_height((avail - 40.0).max(120.0))
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(34.0))
            .column(Column::initial(100.0))
            .column(Column::remainder().at_least(180.0))
            .column(Column::initial(250.0))
            .column(Column::initial(120.0))
            .column(Column::initial(330.0))
            .header(30.0, |mut h| {
                for t in ["", "Scadenza", if kind == BankKind::Sct { "Fornitore" } else { "Cliente" }, "Documento", "Importo", if kind == BankKind::Sdd { "Conto e mandato" } else { "Banca" }] {
                    h.col(|ui| {
                        ui.label(theme::medium(t).size(13.0).color(theme::MUTED));
                    });
                }
            })
            .body(|body| {
                body.rows(42.0, list.len(), |mut row| {
                    let i = row.index();
                    let d = &list[i];
                    let mut on = st.sel.contains(&d.id);
                    row.set_selected(on);
                    row.col(|ui| {
                        if ui.checkbox(&mut on, "").changed() {
                            toggle = Some(d.id);
                        }
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(d.date.format("%d/%m/%Y").to_string()).color(if d.date < today { theme::NEGATIVE } else { theme::INK }));
                    });
                    row.col(|ui| {
                        ui.label(theme::medium(&d.party_name));
                    });
                    row.col(|ui| {
                        if ui.link(&d.doc_label).clicked() {
                            cx.nav.push(Nav::OpenDocument(d.document_id));
                        }
                    });
                    row.col(|ui| {
                        ui.label(theme::medium(format!("€ {}", eur(d.residual()))));
                    });
                    row.col(|ui| match (&problems[i], &parties[i]) {
                        (Some(pr), p) => {
                            ui.label(RichText::new(format!("{}  {}", icon::WARNING_CIRCLE, pr)).color(theme::NEGATIVE));
                            if let Some(p) = p {
                                if ui.small_button("Completa").clicked() {
                                    fix = Some((p.role, p.id, p.display_name()));
                                }
                            }
                        }
                        (None, _) if past(d) => {
                            ui.label(RichText::new(format!("{}  scadenza passata, non presentabile", icon::WARNING)).color(theme::WARNING))
                                .on_hover_text("Incassare in altro modo (bonifico, contanti) o inviare un sollecito dallo scadenzario");
                        }
                        (None, Some(p)) => {
                            ui.label(theme::muted(bank_text(kind, p)));
                        }
                        _ => {}
                    });
                    if row.response().clicked() {
                        toggle = Some(d.id);
                    }
                });
            });
    });
    if let Some(id) = toggle {
        if !st.sel.remove(&id) {
            st.sel.insert(id);
        }
    }
    if let Some((role, id, name)) = fix {
        cx.nav.push(Nav::FindParty(if role == PartyRole::Supplier { PartyRole::Supplier } else { PartyRole::Customer }, id, name));
    }
}

fn create(cx: &mut Ctx, st: &mut State, kind: BankKind, dues: &[DueRow]) {
    let p = match bankfiles::prepare(cx.store, kind, dues, st.execution) {
        Ok(p) => p,
        Err(list) => {
            st.problems = Some(list);
            return;
        }
    };
    let ext = if kind == BankKind::Riba { "txt" } else { "xml" };
    let Some(path) = rfd::FileDialog::new().set_title("Salva il file per l'home banking").add_filter(if kind == BankKind::Riba { "Ri.Ba. CBI" } else { "XML CBI" }, &[ext]).set_file_name(&p.file_name).save_file() else {
        return;
    };
    if let Err(e) = std::fs::write(&path, p.content.as_bytes()) {
        cx.msgs.err(format!("{}: {}", path.display(), e));
        return;
    }
    match bankfiles::issue(cx.store, &p) {
        Ok(b) => {
            cx.msgs.ok(format!("{} creata: {} voci, € {}. File salvato in {}", b.label(), p.entries, eur(p.total), path.display()));
            if !p.warnings.is_empty() {
                cx.msgs.info_dialog("Distinta creata con avvisi", p.warnings.join("\n"));
            }
            st.tab = Tab::Batches;
            st.open_batch = Some(b.id);
            st.sel_for = None;
        }
        Err(e) => cx.msgs.err(format!("Il file è stato salvato ma la distinta non è stata registrata: {}", e)),
    }
}

fn status_pill(ui: &mut egui::Ui, b: &BankBatch) {
    let (t, tone) = match b.status {
        BatchStatus::Cancelled => ("Annullata", Tone::Neutral),
        BatchStatus::Issued if b.unpaid_items > 0 => ("Con insoluti", Tone::Negative),
        BatchStatus::Issued if b.open_items == 0 => (if b.kind == BankKind::Sct { "Pagata" } else { "Incassata" }, Tone::Positive),
        BatchStatus::Issued => (if b.kind == BankKind::Sct { "Da registrare" } else { "Presentata" }, Tone::Warning),
    };
    theme::pill(ui, t, tone);
}

fn batches_view(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, list: &[BankBatch]) {
    if list.is_empty() {
        theme::card(ui, "Nessuna distinta emessa", |_| {}, |ui| {
            ui.label(theme::muted("Le distinte create dalle schede Ri.Ba., SDD e bonifici compaiono qui, con il file, la stampa e la registrazione degli incassi."));
        });
        return;
    }
    let mut toggle = None;
    let settings = cx.store.bank_settings().unwrap_or_default();
    let h = (ui.available_height() * if st.open_batch.is_some() { 0.45 } else { 1.0 } - 40.0).max(140.0);
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        TableBuilder::new(ui)
            .id_salt("batches")
            .striped(false)
            .resizable(false)
            .sense(egui::Sense::click())
            .max_scroll_height(h)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(170.0))
            .column(Column::initial(100.0))
            .column(Column::initial(100.0))
            .column(Column::initial(60.0))
            .column(Column::initial(120.0))
            .column(Column::initial(130.0))
            .column(Column::remainder().at_least(300.0))
            .header(30.0, |mut hd| {
                for t in ["Distinta", "Creata il", "Data", "Voci", "Totale", "Stato", ""] {
                    hd.col(|ui| {
                        ui.label(theme::medium(t).size(13.0).color(theme::MUTED));
                    });
                }
            })
            .body(|body| {
                body.rows(44.0, list.len(), |mut row| {
                    let b = &list[row.index()];
                    row.set_selected(st.open_batch == Some(b.id));
                    row.col(|ui| {
                        ui.label(theme::medium(b.label()));
                    });
                    row.col(|ui| {
                        ui.label(b.created_at.format("%d/%m/%Y").to_string());
                    });
                    row.col(|ui| {
                        ui.label(b.date.format("%d/%m/%Y").to_string()).on_hover_text(if b.kind == BankKind::Sct { "data di esecuzione" } else { "prima scadenza" });
                    });
                    row.col(|ui| {
                        ui.label(b.count.to_string());
                    });
                    row.col(|ui| {
                        ui.label(theme::medium(format!("€ {}", eur(b.total))));
                    });
                    row.col(|ui| status_pill(ui, b));
                    row.col(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if theme::ghost(ui, true, icon::FILE_ARROW_DOWN, "File").on_hover_text("Salva di nuovo il file per l'home banking").clicked() {
                            let ext = if b.kind == BankKind::Riba { "txt" } else { "xml" };
                            if let Some(p) = rfd::FileDialog::new().set_file_name(&b.file_name).add_filter("File", &[ext]).save_file() {
                                match std::fs::write(&p, b.content.as_bytes()) {
                                    Ok(_) => cx.msgs.ok(format!("File salvato in {}", p.display())),
                                    Err(e) => cx.msgs.err(e.to_string()),
                                }
                            }
                        }
                        if theme::ghost(ui, true, icon::FILE_PDF, "Stampa").clicked() {
                            if let Ok((bb, items)) = cx.store.batch(b.id) {
                                if let Err(e) = services::open_bytes(&format!("Distinta {} {}-{}.pdf", b.kind.code(), b.year, b.number), &bankfiles::batch_pdf(cx.store, &bb, &items)) {
                                    cx.msgs.err(e);
                                }
                            }
                        }
                        if b.status == BatchStatus::Issued && b.open_items > 0 {
                            let label = if b.kind == BankKind::Sct { "Registra pagamenti" } else { "Registra incassi" };
                            if theme::ghost(ui, true, icon::HAND_COINS, label).clicked() {
                                let account = if settings.account.trim().is_empty() { "Banca".to_string() } else { settings.account.clone() };
                                let today = chrono::Local::now().date_naive();
                                st.settle = Some((b.id, if b.kind == BankKind::Sct { b.date.min(today) } else { today }, account));
                            }
                        }
                        if b.status == BatchStatus::Issued && theme::ghost_colored(ui, true, icon::PROHIBIT, "Annulla", theme::NEGATIVE).clicked() {
                            st.confirm_cancel = Some(b.id);
                        }
                    });
                    if row.response().clicked() {
                        toggle = Some(b.id);
                    }
                });
            });
    });
    if let Some(id) = toggle {
        st.open_batch = if st.open_batch == Some(id) { None } else { Some(id) };
    }
    let Some(id) = st.open_batch else { return };
    let Ok((b, items)) = cx.store.batch(id) else { return };
    ui.add_space(12.0);
    theme::card(
        ui,
        &format!("Voci della {}", b.label()),
        |ui| {
            ui.label(theme::muted(format!("file {}", b.file_name)).size(12.0));
        },
        |ui| {
            let today = chrono::Local::now().date_naive();
            egui::ScrollArea::vertical().id_salt("batch_items").max_height(ui.available_height() - 10.0).show(ui, |ui| {
                egui::Grid::new("batch_items_grid").num_columns(6).spacing([18.0, 10.0]).striped(true).show(ui, |ui| {
                    for t in ["Scadenza", "Soggetto", "Documento", "Importo", "Stato", ""] {
                        ui.label(theme::medium(t).size(13.0).color(theme::MUTED));
                    }
                    ui.end_row();
                    for it in &items {
                        ui.label(it.due.date.format("%d/%m/%Y").to_string());
                        ui.label(&it.due.party_name);
                        if ui.link(&it.due.doc_label).clicked() {
                            cx.nav.push(Nav::OpenDocument(it.due.document_id));
                        }
                        ui.label(format!("€ {}", eur(it.amount)));
                        let (t, tone) = if it.unpaid {
                            ("Insoluta", Tone::Negative)
                        } else if it.due.residual() <= Decimal::ZERO {
                            (if b.kind == BankKind::Sct { "Pagata" } else { "Incassata" }, Tone::Positive)
                        } else if it.due.date < today {
                            ("Scaduta, da registrare", Tone::Warning)
                        } else {
                            ("In scadenza", Tone::Neutral)
                        };
                        theme::pill(ui, t, tone);
                        if b.kind != BankKind::Sct && b.status == BatchStatus::Issued && !it.unpaid {
                            if theme::ghost_colored(ui, true, icon::ARROW_U_UP_LEFT, "Insoluto", theme::NEGATIVE).on_hover_text("La banca ha comunicato il mancato pagamento").clicked() {
                                st.confirm_unpaid = Some((it.id, format!("{} di {} (€ {})", it.due.doc_label, it.due.party_name, eur(it.amount))));
                            }
                        } else {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                });
            });
        },
    );
}

// ----------------------------------------------------------------------
// Impostazioni
// ----------------------------------------------------------------------

pub fn settings_tab(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut Option<BankSettings>) {
    let s = st.get_or_insert_with(|| cx.store.bank_settings().unwrap_or_default());
    let c = cx.store.company().unwrap_or_default();
    ui.label("Codici per creare i file da caricare nell'home banking: li comunica la banca insieme al servizio di remote banking (CBI).");
    ui.add_space(8.0);
    egui::Grid::new("bank_cfg").num_columns(3).spacing([12.0, 10.0]).show(ui, |ui| {
        ui.label("Conto dell'azienda");
        let ok = openfatt_bank::it_bank_codes(&c.iban);
        ui.label(theme::medium(if c.iban.is_empty() { "IBAN non indicato".to_string() } else { format!("{}  {}", c.iban, c.bank_name) }));
        ui.label(match &ok {
            Some((abi, cab, _)) => RichText::new(format!("ABI {} · CAB {} (si modifica in Azienda)", abi, cab)).color(theme::MUTED),
            None => RichText::new("serve un IBAN italiano valido in Impostazioni > Azienda").color(theme::NEGATIVE),
        });
        ui.end_row();
        ui.label("Codice SIA");
        ui.add(egui::TextEdit::singleline(&mut s.sia).desired_width(90.0).char_limit(5).hint_text("A1B2C"));
        ui.label(theme::muted("5 caratteri, per le Ri.Ba."));
        ui.end_row();
        ui.label("Codice CUC");
        ui.add(egui::TextEdit::singleline(&mut s.cuc).desired_width(160.0).char_limit(35));
        ui.label(theme::muted("Codice Unico CBI, per bonifici e addebiti SDD"));
        ui.end_row();
        ui.label("Identificativo creditore (CID)");
        let valid = s.creditor_id.trim().is_empty() || openfatt_bank::creditor_id_valid(&s.creditor_id);
        ui.add(egui::TextEdit::singleline(&mut s.creditor_id).desired_width(220.0).char_limit(35).text_color(if valid { theme::INK } else { theme::NEGATIVE }).hint_text("IT00ZZZ01234567890"));
        ui.label(if valid { theme::muted("per gli addebiti SDD") } else { RichText::new("cifre di controllo errate").color(theme::NEGATIVE) });
        ui.end_row();
        ui.label("Conto di prima nota");
        let accounts = cx.store.accounts().unwrap_or_default();
        egui::ComboBox::from_id_salt("bank_account").selected_text(if s.account.is_empty() { "Banca".to_string() } else { s.account.clone() }).show_ui(ui, |ui| {
            for a in &accounts {
                ui.selectable_value(&mut s.account, a.clone(), a);
            }
        });
        ui.label(theme::muted("dove registrare incassi e pagamenti delle distinte"));
        ui.end_row();
    });
    ui.add_space(10.0);
    if theme::primary(ui, icon::FLOPPY_DISK, "Salva").clicked() {
        s.sia = s.sia.trim().to_ascii_uppercase();
        s.creditor_id = openfatt_bank::normalize_iban(&s.creditor_id);
        cx.msgs.check(cx.store.set_bank_settings(s), "Codici della banca salvati");
    }
    ui.add_space(14.0);
    ui.label(theme::muted("Formati prodotti: Ri.Ba. nel tracciato CBI a 120 caratteri; addebiti diretti SEPA (Core e B2B) nel formato XML CBI SDD 00.01.01; bonifici SEPA nel formato XML CBI CBIPaymentRequest 00.04.01 (in vigore dal 17/03/2024). Se la banca chiede un formato diverso, verificare con la banca prima del caricamento."));
}
