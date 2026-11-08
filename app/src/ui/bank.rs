//! Distinte bancarie: Ri.Ba. e addebiti SDD da presentare, bonifici da disporre,
//! distinte emesse con incassi, insoluti e annullo.

use std::collections::BTreeSet;

use chrono::NaiveDate;
use eframe::egui::{self, RichText};
use openfatt_core::fiscal;
use openfatt_core::model::{Party, PartyRole};
use openfatt_store::{BankBatch, BankKind, BankSettings, BatchStatus, DueRow};
use rust_decimal::Decimal;

use super::grid::{Col, Grid, Val};
use super::quick::Ref;
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
    /// Voce corrente dell'elenco (l'ultima cliccata).
    current: Option<i64>,
    /// Operazione in attesa della scelta delle voci.
    ask: Option<BankOp>,
    grid: Grid,
    batch_grid: Grid,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BankOp {
    Export,
    Print,
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
            current: None,
            ask: None,
            grid: {
                let mut g = Grid::new("banca-voci");
                g.keep_selection = true;
                g
            },
            batch_grid: Grid::new("banca-distinte"),
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
        return Some("Manca l'IBAN italiano dell'azienda (Opzioni > Azienda).");
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
    let title = match st.tab {
        Tab::Riba => "RIBA da emettere",
        Tab::Sdd => "Addebiti SDD da emettere",
        Tab::Sct => "Bonifici da pagare",
        Tab::Batches => "Distinte emesse",
    };
    let _ = sub;
    theme::title_row(ui, &[], title, None, |ui| {
        if theme::ghost(ui, true, icon::GEAR, "Codici della banca").clicked() {
            cx.nav.push(Nav::BankSettings);
        }
    });
    let _ = (&count, &batches);
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
    // voci che la banca può accettare
    let visible: Vec<DueRow> = list.iter().zip(&problems).filter(|(d, p)| p.is_none() && !past(d)).map(|(d, _)| d.clone()).collect();
    ui.horizontal(|ui| {
        let has = !visible.is_empty();
        if theme::primary_enabled(ui, has, icon::FILE_ARROW_DOWN, "Esporta file per home banking").on_hover_text("File CBI da caricare nel portale della banca (InBank e altri): la distinta viene registrata").clicked() {
            st.ask = Some(BankOp::Export);
        }
        if theme::button_enabled(ui, has, icon::PRINTER, "Stampa distinta").on_hover_text("Solo stampa: non registra la distinta").clicked() {
            st.ask = Some(BankOp::Print);
        }
        ui.label(theme::muted(format!("{} spuntate · € {}", selected.len(), eur(sel_total))));
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
    let index: std::collections::HashMap<i64, usize> = list.iter().enumerate().map(|(i, d)| (d.id, i)).collect();
    let (index_ref, problems_ref, parties_ref) = (&index, &problems, &parties);
    let fix_req: std::cell::Cell<Option<(PartyRole, i64, String)>> = std::cell::Cell::new(None);
    let open_req = std::cell::Cell::new(None);
    let (fix_ref, open_ref) = (&fix_req, &open_req);
    let cols: Vec<Col<DueRow>> = vec![
        Col::new("date", "Scadenza", 100.0, |d: &DueRow| Val::Date(Some(d.date))).render(move |ui: &mut egui::Ui, d: &DueRow| {
            ui.label(RichText::new(d.date.format("%d/%m/%Y").to_string()).color(if d.date < today { theme::NEGATIVE } else { theme::INK }));
        }),
        Col::text("party", if kind == BankKind::Sct { "Fornitore" } else { "Cliente" }, 280.0, |d: &DueRow| d.party_name.clone()).group().opens(|d: &DueRow| d.party_id.map(Ref::Party)),
        Col::text("doc", "Documento", 250.0, |d: &DueRow| d.doc_label.clone()).render(move |ui: &mut egui::Ui, d: &DueRow| {
            if ui.link(&d.doc_label).clicked() {
                open_ref.set(Some(d.document_id));
            }
            if d.draft {
                theme::pill(ui, "bozza", Tone::Warning);
            }
        }),
        Col::money("amount", "Importo", 120.0, |d: &DueRow| d.residual()).total(),
        Col::text("bank", if kind == BankKind::Sdd { "Conto e mandato" } else { "Banca" }, 330.0, move |d: &DueRow| index_ref.get(&d.id).and_then(|i| parties_ref[*i].as_ref()).map(|p| bank_text(kind, p)).unwrap_or_default()).render(move |ui: &mut egui::Ui, d: &DueRow| {
            let Some(i) = index_ref.get(&d.id).copied() else { return };
            match (&problems_ref[i], &parties_ref[i]) {
                (Some(pr), p) => {
                    ui.label(RichText::new(format!("{}  {}", icon::WARNING_CIRCLE, pr)).color(theme::NEGATIVE));
                    if let Some(p) = p {
                        if ui.small_button("Completa").clicked() {
                            fix_ref.set(Some((p.role, p.id, p.display_name())));
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
            }
        }),
    ];
    let visible: Vec<DueRow> = visible.into_iter().filter(|d| st.grid.accepts(&cols, d)).collect();
    // scelta delle voci su cui eseguire l'operazione
    if let Some(op) = st.ask {
        let mut chosen: Option<Vec<DueRow>> = None;
        let mut close = false;
        let cur: Option<DueRow> = st.current.and_then(|id| visible.iter().find(|d| d.id == id)).cloned();
        let sel_ok: Vec<DueRow> = visible.iter().filter(|d| st.sel.contains(&d.id)).cloned().collect();
        egui::Window::new("Conferma").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
            ui.label(RichText::new("Su che voci vuoi eseguire l'operazione?").size(14.0));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if theme::button_enabled(ui, cur.is_some(), "", "Voce corrente").clicked() {
                    chosen = cur.clone().map(|d| vec![d]);
                }
                if !sel_ok.is_empty() && sel_ok.len() != visible.len() && theme::button(ui, "", &format!("Voci spuntate ({})", sel_ok.len())).clicked() {
                    chosen = Some(sel_ok.clone());
                }
                if theme::button(ui, "", &format!("Voci visibili ({})", visible.len())).clicked() {
                    chosen = Some(visible.clone());
                }
                if theme::button(ui, "", "Annulla").clicked() {
                    close = true;
                }
            });
        });
        if let Some(dues) = chosen {
            st.ask = None;
            match op {
                BankOp::Export => create(cx, st, kind, &dues),
                BankOp::Print => {
                    let pdf = bankfiles::preview_pdf(cx.store, kind, &dues);
                    if let Err(e) = services::open_bytes("Distinta.pdf", &pdf) {
                        cx.msgs.err(e);
                    }
                }
            }
        } else if close {
            st.ask = None;
        }
    }
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            st.grid.toolbar(ui, cx.store, cx.msgs, &cols, &list, "Voci della distinta");
        });
    });
    let avail = ui.available_height();
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        st.grid.selected = st.sel.clone();
        st.grid.version = 0;
        st.grid.show(ui, cx.store, &cols, &list, |d| d.id, (avail - 40.0).max(120.0), true, 26.0);
        st.sel = st.grid.selected.clone();
        st.current = st.grid.anchor();
    });
    if let Some(d) = open_req.get() {
        cx.nav.push(Nav::OpenDocument(d));
    }
    if let Some((role, id, name)) = fix_req.take() {
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
    let settings = cx.store.bank_settings().unwrap_or_default();
    #[derive(Clone, Copy)]
    enum Act {
        File(i64),
        Print(i64),
        Settle(i64),
        Cancel(i64),
    }
    let action = std::cell::Cell::new(None);
    let act = &action;
    let cols: Vec<Col<BankBatch>> = vec![
        Col::text("label", "Distinta", 170.0, |b: &BankBatch| b.label()),
        Col::text("kind", "Tipo", 80.0, |b: &BankBatch| b.kind.code().to_string()).hidden().group(),
        Col::new("created", "Creata il", 100.0, |b: &BankBatch| Val::Date(Some(b.created_at.date()))),
        Col::new("date", "Data", 100.0, |b: &BankBatch| Val::Date(Some(b.date))),
        Col::new("count", "Voci", 60.0, |b: &BankBatch| Val::Num(Decimal::from(b.count as i64), 0)).right(),
        Col::money("total", "Totale", 120.0, |b: &BankBatch| b.total).total(),
        Col::text("status", "Stato", 130.0, |b: &BankBatch| match b.status {
            BatchStatus::Cancelled => "Annullata".to_string(),
            BatchStatus::Issued if b.unpaid_items > 0 => "Con insoluti".to_string(),
            BatchStatus::Issued if b.open_items == 0 => "Chiusa".to_string(),
            BatchStatus::Issued => "Aperta".to_string(),
        }).group().render(|ui: &mut egui::Ui, b: &BankBatch| status_pill(ui, b)),
        Col::text("act", "", 420.0, |_: &BankBatch| String::new()).render(move |ui: &mut egui::Ui, b: &BankBatch| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if theme::ghost(ui, true, icon::FILE_ARROW_DOWN, "File").on_hover_text("Salva di nuovo il file per l'home banking").clicked() {
                act.set(Some(Act::File(b.id)));
            }
            if theme::ghost(ui, true, icon::FILE_PDF, "Stampa").clicked() {
                act.set(Some(Act::Print(b.id)));
            }
            if b.status == BatchStatus::Issued && b.open_items > 0 {
                let label = if b.kind == BankKind::Sct { "Registra pagamenti" } else { "Registra incassi" };
                if theme::ghost(ui, true, icon::HAND_COINS, label).clicked() {
                    act.set(Some(Act::Settle(b.id)));
                }
            }
            if b.status == BatchStatus::Issued && theme::ghost_colored(ui, true, icon::PROHIBIT, "Annulla", theme::NEGATIVE).clicked() {
                act.set(Some(Act::Cancel(b.id)));
            }
        }),
    ];
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            st.batch_grid.toolbar(ui, cx.store, cx.msgs, &cols, list, "Distinte emesse");
        });
    });
    let h = (ui.available_height() * if st.open_batch.is_some() { 0.45 } else { 1.0 } - 40.0).max(140.0);
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        st.batch_grid.version = 0;
        st.batch_grid.show(ui, cx.store, &cols, list, |b| b.id, h, false, 30.0);
    });
    st.open_batch = st.batch_grid.single();
    match action.get() {
        Some(Act::File(id)) => {
            if let Some(b) = list.iter().find(|b| b.id == id) {
                let ext = if b.kind == BankKind::Riba { "txt" } else { "xml" };
                if let Some(p) = rfd::FileDialog::new().set_file_name(&b.file_name).add_filter("File", &[ext]).save_file() {
                    match std::fs::write(&p, b.content.as_bytes()) {
                        Ok(_) => cx.msgs.ok(format!("File salvato in {}", p.display())),
                        Err(e) => cx.msgs.err(e.to_string()),
                    }
                }
            }
        }
        Some(Act::Print(id)) => {
            if let Ok((bb, items)) = cx.store.batch(id) {
                if let Err(e) = services::open_bytes(&format!("Distinta {} {}-{}.pdf", bb.kind.code(), bb.year, bb.number), &bankfiles::batch_pdf(cx.store, &bb, &items)) {
                    cx.msgs.err(e);
                }
            }
        }
        Some(Act::Settle(id)) => {
            if let Some(b) = list.iter().find(|b| b.id == id) {
                let account = if settings.account.trim().is_empty() { "Banca".to_string() } else { settings.account.clone() };
                let today = chrono::Local::now().date_naive();
                st.settle = Some((b.id, if b.kind == BankKind::Sct { b.date.min(today) } else { today }, account));
            }
        }
        Some(Act::Cancel(id)) => st.confirm_cancel = Some(id),
        None => {}
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
            None => RichText::new("serve un IBAN italiano valido in Opzioni > Azienda").color(theme::NEGATIVE),
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
