//! Elenchi dei documenti con filtri, selezione multipla e operazioni.

use std::collections::BTreeSet;

use chrono::Datelike;
use eframe::egui::{self, RichText, Vec2};
use openfatt_core::model::{DocKind, DocStatus};
use openfatt_store::{DocFilter, DocSummary};
use rust_decimal::Decimal;

use super::grid::{Col, Grid, Val};
use super::theme::{self, icon, Tone, MUTED};
use super::widgets::{self, eur, ORANGE, RED};
use super::{Ctx, DocGroup, Nav};
use crate::services;

pub struct State {
    pub dirty: bool,
    search: String,
    year: Option<i32>,
    year_checked: bool,
    selected: BTreeSet<i64>,
    last_group: Option<DocGroup>,
    confirm: Option<(String, Action)>,
    issues: Option<(String, Vec<openfatt_fatturapa::Issue>)>,
    status: Option<DocStatus>,
    quick: Quick,
    grids: Vec<(DocGroup, Grid)>,
    data: super::Cache<(String, Option<i32>, String, Option<&'static str>), Vec<DocSummary>>,
    progress: super::Cache<String, std::collections::HashMap<i64, openfatt_store::OrderProgress>>,
}

fn grid_name(g: DocGroup) -> &'static str {
    match g {
        DocGroup::Quotes => "doc-preventivi",
        DocGroup::Orders => "doc-ordini",
        DocGroup::DeliveryNotes => "doc-ddt",
        DocGroup::Invoices => "doc-fatture",
        DocGroup::Proforma => "doc-proforma",
        DocGroup::SelfInvoices => "doc-autofatture",
        DocGroup::SupplierQuotes => "doc-preventivi-fornitori",
        DocGroup::SupplierOrders => "doc-ordini-fornitori",
        DocGroup::GoodsReceipts => "doc-arrivi",
        DocGroup::PurchaseInvoices => "doc-acquisti",
        DocGroup::Production => "doc-produzione",
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quick {
    All,
    Open,
    ToSend,
    Drafts,
}

impl Default for State {
    fn default() -> Self {
        State {
            dirty: true,
            search: String::new(),
            year: Some(chrono::Local::now().year()),
            year_checked: false,
            selected: BTreeSet::new(),
            last_group: None,
            confirm: None,
            issues: None,
            status: None,
            quick: Quick::All,
            grids: Vec::new(),
            data: Default::default(),
            progress: Default::default(),
        }
    }
}

#[derive(Clone)]
enum Action {
    Delete(Vec<i64>),
    Cancel(Vec<i64>),
}

/// Genera la fattura elettronica di uno o più documenti emessi.
pub fn generate_einvoices(cx: &mut Ctx, ids: &[i64], issues_out: &mut Option<(String, Vec<openfatt_fatturapa::Issue>)>) {
    let mut ok = 0;
    let mut errors = Vec::new();
    for id in ids {
        match services::create_einvoice(cx.store, *id) {
            Ok((e, issues)) => {
                ok += 1;
                let warns: Vec<_> = issues.into_iter().filter(|i| i.severity == openfatt_fatturapa::Severity::Warning).collect();
                if !warns.is_empty() && ids.len() == 1 {
                    *issues_out = Some((format!("{} generata con avvisi", e.file_name), warns));
                }
            }
            Err(e) => errors.push(e),
        }
    }
    if ok > 0 {
        cx.msgs.ok(format!("{} fatture elettroniche generate: si inviano da 'Fatture elettr. > Fatture inviate'", ok));
    }
    if !errors.is_empty() {
        cx.msgs.err(errors.join("\n\n"));
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, group: DocGroup) {
    if st.last_group != Some(group) {
        st.selected.clear();
        st.last_group = Some(group);
    }
    let kinds = group.kinds();

    if let Some((text, action)) = st.confirm.clone() {
        match widgets::confirm(ui.ctx(), "Conferma", &text) {
            Some(true) => {
                match action {
                    Action::Delete(ids) => {
                        for id in ids {
                            cx.msgs.check(cx.store.delete_document(id), "Documento eliminato");
                        }
                    }
                    Action::Cancel(ids) => {
                        for id in ids {
                            cx.msgs.check(cx.store.cancel_document(id), "Documento annullato");
                        }
                    }
                }
                st.selected.clear();
                st.confirm = None;
            }
            Some(false) => st.confirm = None,
            None => {}
        }
    }

    if let Some((title, issues)) = &st.issues {
        let mut close = false;
        egui::Window::new(title.as_str()).collapsible(false).default_width(640.0).show(ui.ctx(), |ui| {
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for i in issues {
                    ui.label(RichText::new(i.to_string()).color(if i.severity == openfatt_fatturapa::Severity::Error { RED } else { ORANGE }));
                }
            });
            if ui.button("Chiudi").clicked() {
                close = true;
            }
        });
        if close {
            st.issues = None;
        }
    }

    let years = cx.store.document_years().unwrap_or_default();
    // archivio senza documenti nell'anno corrente (es. appena importato): si parte dall'ultimo anno con documenti
    if !st.year_checked {
        st.year_checked = true;
        if let (Some(y), Some(last)) = (st.year, years.iter().max()) {
            if !years.contains(&y) && *last < y {
                st.year = Some(*last);
            }
        }
    }
    let filter = DocFilter { kinds: kinds.clone(), year: st.year, search: st.search.clone(), status: st.status, ..Default::default() };
    let fkey = (format!("{:?}", filter.kinds), filter.year, filter.search.clone(), filter.status.map(|s| s.code()));
    let all_rc = st.data.get(cx.store.generation(), &fkey, || cx.store.documents(&filter).unwrap_or_default());
    let all: &Vec<DocSummary> = &all_rc;
    let fiscal_group = kinds.iter().any(|k| k.is_fiscal());
    let purchase = DocGroup::PURCHASES.contains(&group);
    let needs_fe = |d: &DocSummary| !purchase && d.kind.is_fiscal() && d.status == DocStatus::Issued && (d.fe_status.is_none() || matches!(d.fe_status.as_deref(), Some("SCARTATA") | Some("DA_INVIARE")));
    let count_open = all.iter().filter(|d| d.open_amount > Decimal::ZERO).count();
    let count_send = all.iter().filter(|d| needs_fe(d)).count();
    let count_drafts = all.iter().filter(|d| d.status == DocStatus::Draft).count();
    let list: Vec<&DocSummary> = all
        .iter()
        .filter(|d| match st.quick {
            Quick::All => true,
            Quick::Open => d.open_amount > Decimal::ZERO,
            Quick::ToSend => needs_fe(d),
            Quick::Drafts => d.status == DocStatus::Draft,
        })
        .collect();
    st.selected.retain(|id| list.iter().any(|d| d.id == *id));
    let selected: Vec<&DocSummary> = list.iter().copied().filter(|d| st.selected.contains(&d.id)).collect();

    // ---- titolo ----
    let open: Decimal = all.iter().map(|d| d.open_amount).sum();
    let open_txt = format!("€ {} {}", eur(open), if purchase { "da pagare" } else { "da incassare" });
    let n_txt = if all.len() == 1 { "1 voce".to_string() } else { format!("{} voci", all.len()) };
    let mut parts: Vec<(&str, bool)> = vec![(&n_txt, false)];
    if open > Decimal::ZERO {
        parts.push((" · ", false));
        parts.push((&open_txt, true));
    }
    theme::title_row(ui, &[], group.label(), Some(theme::subline(&parts)), |ui| {
        if group == DocGroup::SelfInvoices {
            egui::menu::menu_custom_button(ui, theme::button_widget(icon::PLUS, "Nuova autofattura"), |ui| {
                for (code, desc) in openfatt_core::codes::TIPO_DOCUMENTO.iter().filter(|(c, _)| ("TD16".."TD30").contains(c) && !matches!(*c, "TD24" | "TD25")) {
                    if ui.button(format!("{}  {}", code, desc)).clicked() {
                        let mut d = openfatt_core::model::Document::new(DocKind::SelfInvoice);
                        d.fe_type = code.to_string();
                        cx.nav.push(Nav::EditDocument(d));
                        ui.close_menu();
                    }
                }
            });
        } else {
            if theme::primary(ui, icon::PLUS, &new_label(kinds[0])).on_hover_text("Ctrl+N").clicked() {
                cx.nav.push(Nav::NewDocument(kinds[0]));
            }
            if kinds.len() > 1 {
                egui::menu::menu_custom_button(ui, theme::button_widget(icon::CARET_DOWN, "Altri"), |ui| {
                    for k in kinds.iter().skip(1) {
                        if ui.button(format!("{}  {}", icon::PLUS, k.label())).clicked() {
                            cx.nav.push(Nav::NewDocument(*k));
                            ui.close_menu();
                        }
                    }
                });
            }
        }
    });

    theme::card_frame().inner_margin(0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let show_kind = kinds.len() > 1;
        let party_title: &'static str = if purchase { "Fornitore" } else { "Cliente" };
        let internal = group == DocGroup::Production;
        let open_title: &'static str = if purchase { "Da pagare" } else { "Da incassare" };
        let orders = kinds.iter().all(|k| k.has_order_state());
        let has_dues = kinds.iter().any(|k| k.has_due_dates());
        let prog_rc = st.progress.get(cx.store.generation(), &format!("{:?}", kinds), || if orders { cx.store.order_progress(&kinds).unwrap_or_default() } else { Default::default() });
        let prog: &std::collections::HashMap<i64, openfatt_store::OrderProgress> = &prog_rc;
        let open_req: std::cell::Cell<Option<i64>> = std::cell::Cell::new(None);
        let open_ref = &open_req;
        let today = chrono::Local::now().date_naive();
        let effective = move |d: &DocSummary| {
            let p = prog.get(&d.id);
            d.order_status.effective(p.map(|x| x.any).unwrap_or(false), p.map(|x| x.all).unwrap_or(false))
        };
        let cols: Vec<Col<&DocSummary>> = {
            let mut kind_col = Col::text("kind", "Tipo", 170.0, |d: &&DocSummary| d.kind.label().to_string()).group();
            if !show_kind {
                kind_col = kind_col.hidden();
            }
            let mut v = vec![
                Col::new("date", "Data", 90.0, |d: &&DocSummary| Val::Date(Some(d.date))),
                Col::new("num", "N.", 70.0, |d: &&DocSummary| d.number.map(|n| Val::Num(Decimal::from(n), 0)).unwrap_or(Val::Text(String::new()))).render(|ui: &mut egui::Ui, d: &&DocSummary| {
                    ui.label(if d.number_label.is_empty() { "-".to_string() } else { d.number_label.clone() });
                }),
                kind_col,
                Col::text("month", "Mese", 90.0, |d: &&DocSummary| d.date.format("%Y-%m").to_string()).hidden().group(),
                Col::text("party", party_title, 300.0, |d: &&DocSummary| d.party_name.clone()).group(),
            ];
            if orders {
                v.push(Col::text("order", "Stato", 320.0, move |d: &&DocSummary| effective(d).label().to_string()).group().render(move |ui: &mut egui::Ui, d: &&DocSummary| {
                    let label = effective(d).label();
                    let targets = prog.get(&d.id).map(|p| p.targets.as_slice()).unwrap_or(&[]);
                    match targets {
                        [] => {
                            ui.label(label);
                        }
                        [(id, t)] => {
                            if ui.link(format!("{}:   -> {}", label, t)).clicked() {
                                open_ref.set(Some(*id));
                            }
                        }
                        many => {
                            ui.visuals_mut().button_frame = false;
                            let txt = RichText::new(format!("{}:   -> Documenti...", label)).color(ui.visuals().hyperlink_color);
                            ui.menu_button(txt, |ui| {
                                for (id, t) in many {
                                    if ui.button(t).clicked() {
                                        open_ref.set(Some(*id));
                                        ui.close_menu();
                                    }
                                }
                            });
                        }
                    }
                }));
                v.push(Col::text("comment", "Commento", 240.0, |d: &&DocSummary| d.comment.clone()));
                v.push(Col::new("expected", "Concl. prevista", 110.0, |d: &&DocSummary| Val::Date(d.expected_date)).render(move |ui: &mut egui::Ui, d: &&DocSummary| {
                    if let Some(x) = d.expected_date {
                        let late = x < today && matches!(effective(d), openfatt_core::model::OrderStatus::Partial | openfatt_core::model::OrderStatus::Confirmed);
                        let t = x.format("%d/%m/%Y").to_string();
                        if late {
                            ui.label(theme::bold(t).color(theme::NEGATIVE));
                        } else {
                            ui.label(t);
                        }
                    }
                }));
            }
            v.push(Col::text("ext", "N. fornitore", 110.0, |d: &&DocSummary| d.external_number.clone()).hidden());
            v.push(Col::money("total", "Tot. documento", 110.0, |d: &&DocSummary| d.total).total());
            let mut open_col = Col::money("open", open_title, 110.0, |d: &&DocSummary| d.open_amount).total().render(move |ui: &mut egui::Ui, d: &&DocSummary| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    if d.open_amount > Decimal::ZERO {
                        ui.label(eur(d.open_amount));
                    } else if d.kind.has_due_dates() && d.status == DocStatus::Issued {
                        ui.label(if purchase { "Pagata" } else { "Incassata" });
                    }
                });
            });
            if !has_dues {
                open_col = open_col.hidden();
            }
            v.push(open_col);
            let mut status_col = Col::text("status", "Stato doc.", 80.0, |d: &&DocSummary| d.status.label().to_string()).group().render(|ui: &mut egui::Ui, d: &&DocSummary| {
                let tone = match d.status {
                    DocStatus::Draft => Tone::Warning,
                    DocStatus::Issued => Tone::Neutral,
                    DocStatus::Cancelled => Tone::Negative,
                };
                theme::pill(ui, d.status.label(), tone);
            });
            if orders {
                status_col = status_col.hidden();
            }
            v.push(status_col);
            let mut fe_col = Col::text("fe", if fiscal_group && !purchase { "Fattura elettronica" } else { "Note" }, if fiscal_group { 260.0 } else { 180.0 }, |d: &&DocSummary| d.fe_status.clone().unwrap_or_default())
                .group()
                .render(move |ui: &mut egui::Ui, d: &&DocSummary| {
                    if fiscal_group && !purchase && d.kind.is_fiscal() && d.status == DocStatus::Issued {
                        theme::sdi_steps(ui, d.fe_status.as_deref());
                    } else if !d.external_number.is_empty() {
                        ui.label(RichText::new(format!("n. fornitore {}", d.external_number)).color(MUTED));
                    }
                });
            if orders || !(fiscal_group || purchase) {
                fe_col = fe_col.hidden();
            }
            v.push(fe_col);
            v
        };
        // la produzione non ha cliente né importi
        let cols: Vec<Col<&DocSummary>> = cols.into_iter().map(|c| if internal && matches!(c.id, "party" | "total" | "open") { c.hidden() } else { c }).collect();
        let gi = match st.grids.iter().position(|(g, _)| *g == group) {
            Some(i) => i,
            None => {
                st.grids.push((group, Grid::new(grid_name(group))));
                st.grids.len() - 1
            }
        };
        let grid = &mut st.grids[gi].1;
        // ---- filtri ----
        egui::Frame::new().inner_margin(egui::Margin { left: 6, right: 6, top: 4, bottom: 4 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut st.search).hint_text(format!("{}  Cliente, numero, testo...", icon::MAGNIFYING_GLASS)).desired_width(240.0));
                egui::ComboBox::from_id_salt("doc_year").height(300.0).selected_text(format!("{}  {}", icon::CALENDAR_BLANK, st.year.map(|y| y.to_string()).unwrap_or_else(|| "Tutti gli anni".into()))).show_ui(ui, |ui| {
                    ui.selectable_value(&mut st.year, None, "Tutti gli anni");
                    let mut ys = years.clone();
                    let cur = chrono::Local::now().year();
                    if !ys.contains(&cur) {
                        ys.insert(0, cur);
                    }
                    for y in ys {
                        ui.selectable_value(&mut st.year, Some(y), y.to_string());
                    }
                });
                egui::ComboBox::from_id_salt("doc_status").selected_text(st.status.map(|s| s.label()).unwrap_or("Tutti gli stati")).show_ui(ui, |ui| {
                    ui.selectable_value(&mut st.status, None, "Tutti gli stati");
                    for s in [DocStatus::Draft, DocStatus::Issued, DocStatus::Cancelled] {
                        ui.selectable_value(&mut st.status, Some(s), s.label());
                    }
                });
                ui.add_space(8.0);
                // filtri rapidi
                let mut seg = vec![(Quick::All, "Tutti".to_string(), all.len())];
                if kinds.iter().any(|k| k.has_due_dates()) {
                    seg.push((Quick::Open, if purchase { "Da pagare".into() } else { "Da incassare".into() }, count_open));
                }
                if fiscal_group && !purchase {
                    seg.push((Quick::ToSend, "Da inviare allo SdI".into(), count_send));
                }
                seg.push((Quick::Drafts, "Bozze".into(), count_drafts));
                egui::Frame::new().fill(theme::RAISED).stroke(egui::Stroke::new(1.0_f32, theme::BORDER)).corner_radius(6).inner_margin(2).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (q, label, n) in seg {
                        let on = st.quick == q;
                        let r = ui
                            .scope(|ui| {
                                theme::flat_visuals(ui, if on { theme::SURFACE } else { egui::Color32::TRANSPARENT }, if on { theme::SURFACE } else { theme::ACTIVE }, theme::ACTIVE);
                                let mut job = egui::text::LayoutJob::default();
                                job.append(&label, 0.0, egui::TextFormat { font_id: egui::FontId::new(14.0, theme::medium_family()), color: if on { theme::INK } else { MUTED }, ..Default::default() });
                                job.append(&n.to_string(), 6.0, egui::TextFormat { font_id: egui::FontId::new(12.0, egui::FontFamily::Proportional), color: MUTED, ..Default::default() });
                                ui.add(egui::Button::new(job).min_size(Vec2::new(0.0, 22.0)))
                            })
                            .inner;
                        if r.clicked() {
                            st.quick = q;
                        }
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    grid.toolbar(ui, cx.store, cx.msgs, &cols, &list, group.label());
                });
            });
        });
        theme::rule(ui);

        // ---- barra delle azioni sulla selezione ----
        let bar_fill = if selected.is_empty() { theme::SURFACE } else { egui::Color32::from_rgb(0xf5, 0xf8, 0xfc) };
        egui::Frame::new().fill(bar_fill).inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
            ui.set_min_height(26.0);
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let all_sel = !list.is_empty() && selected.len() == list.len();
                let mut chk = all_sel;
                if ui.checkbox(&mut chk, "").on_hover_text("Seleziona tutti").changed() {
                    if chk {
                        st.selected = list.iter().map(|d| d.id).collect();
                    } else {
                        st.selected.clear();
                    }
                }
                if selected.is_empty() {
                    return;
                }
                let sel_total: Decimal = selected.iter().map(|d| d.total).sum();
                ui.label(format!("{} selezionat{} · € {}", selected.len(), if selected.len() == 1 { "o" } else { "i" }, eur(sel_total)));
                ui.add_space(10.0);
                let one = if selected.len() == 1 { Some(selected[0]) } else { None };
                if theme::ghost(ui, one.is_some(), icon::PENCIL_SIMPLE, "Apri").clicked() {
                    cx.nav.push(Nav::OpenDocument(one.unwrap().id));
                }
                let drafts: Vec<&DocSummary> = selected.iter().copied().filter(|d| d.status == DocStatus::Draft).collect();
                if !drafts.is_empty() && theme::ghost(ui, true, icon::CHECK, "Emetti").clicked() {
                    let mut sorted = drafts.clone();
                    sorted.sort_by_key(|d| (d.date, d.id));
                    for d in sorted {
                        cx.msgs.check(cx.store.issue_document(d.id), "Documento emesso");
                    }
                }
                let fiscal: Vec<i64> = selected.iter().filter(|d| d.kind.is_fiscal() && d.status == DocStatus::Issued && (d.fe_status.is_none() || d.fe_status.as_deref() == Some("SCARTATA"))).map(|d| d.id).collect();
                if !fiscal.is_empty() && !purchase && theme::ghost(ui, true, icon::FILE_ARROW_UP, "Genera fattura elettronica").clicked() {
                    generate_einvoices(cx, &fiscal, &mut st.issues);
                }
                let pdf_label = if selected.len() > 1 { format!("Stampa {} in un PDF", selected.len()) } else { "PDF".to_string() };
                if theme::ghost(ui, true, icon::FILE_PDF, &pdf_label).on_hover_text("Più documenti: un unico PDF, ciascuno da una nuova pagina").clicked() {
                    let ids: Vec<i64> = selected.iter().map(|d| d.id).collect();
                    let name = match one {
                        Some(d) => format!("{}_{}.pdf", d.kind.code(), d.number_label.replace('/', "-")),
                        None => format!("{} documenti.pdf", ids.len()),
                    };
                    if let Err(e) = crate::printing::documents_pdf(cx.store, &ids).and_then(|pdf| services::open_bytes(&name, &pdf)) {
                        cx.msgs.err(e);
                    }
                }
                if !purchase && theme::ghost(ui, true, icon::ENVELOPE_SIMPLE, "Invia per email").on_hover_text("Un messaggio per ogni documento, al cliente del documento, con il PDF allegato").clicked() {
                    let mut list = Vec::new();
                    for d in &selected {
                        match cx.store.document(d.id).map_err(|e| e.to_string()).and_then(|doc| crate::mailer::for_document(cx.store, &doc)) {
                            Ok(c) => list.push(c),
                            Err(e) => cx.msgs.err(format!("{}: {}", d.number_label, e)),
                        }
                    }
                    cx.nav.push(Nav::Mail(list));
                }
                if theme::ghost(ui, one.is_some(), icon::COPY, "Duplica").clicked() {
                    if let Some(d) = cx.msgs.check(cx.store.duplicate_document(one.unwrap().id, None), "Copia creata") {
                        cx.nav.push(Nav::OpenDocument(d.id));
                    }
                }
                // trasformazioni (anche da più documenti dello stesso tipo)
                let k0 = selected[0].kind;
                if selected.iter().all(|d| d.kind == k0) {
                    for t in k0.transform_targets() {
                        let label = if selected.len() > 1 { format!("{} da {} documenti", t.label(), selected.len()) } else { format!("{}", t.label()) };
                        if theme::ghost(ui, true, icon::ARROW_RIGHT, &label).clicked() {
                            let ids: Vec<i64> = selected.iter().map(|d| d.id).collect();
                            if let Some(d) = cx.msgs.check(cx.store.transform(&ids, *t), "") {
                                cx.msgs.ok(format!("{} creato in bozza", t.label()));
                                cx.nav.push(Nav::OpenDocument(d.id));
                            }
                        }
                    }
                }
                if theme::ghost(ui, true, icon::PROHIBIT, "Annulla").on_hover_text("Annulla il documento: resta in archivio con il suo numero").clicked() {
                    st.confirm = Some((
                        format!("Annullare {} documenti? Resteranno in archivio con il loro numero, senza effetti su magazzino e scadenze.", selected.len()),
                        Action::Cancel(selected.iter().map(|d| d.id).collect()),
                    ));
                }
                if theme::ghost_colored(ui, true, icon::TRASH, "Elimina", theme::NEGATIVE).clicked() {
                    st.confirm = Some((format!("Eliminare definitivamente {} documenti?", selected.len()), Action::Delete(selected.iter().map(|d| d.id).collect())));
                }
            });
        });
        theme::rule(ui);

        // ---- tabella ----
        if list.is_empty() {
            egui::Frame::new().inner_margin(24).show(ui, |ui| {
                ui.label(theme::muted(if all.is_empty() { "Nessun documento con questi filtri. Usa il pulsante in alto a destra per crearne uno." } else { "Nessun documento in questa vista." }));
            });
            return;
        }
        grid.selected = st.selected.clone();
        let avail = ui.available_height() - 4.0;
        grid.version = st.data.version.wrapping_mul(7).wrapping_add(st.quick as u64 + 1);
        let ev = grid.show(ui, cx.store, &cols, &list, |d| d.id, avail, false, 24.0);
        st.selected = grid.selected.clone();
        if let Some(id) = open_req.get() {
            cx.nav.push(Nav::OpenDocument(id));
        }
        if let Some(id) = ev.opened {
            cx.nav.push(Nav::OpenDocument(id));
        }
    });
}

/// Testo del pulsante "Nuovo/Nuova ..." con l'articolo giusto.
fn new_label(k: DocKind) -> String {
    let l = k.label();
    let fem = l.starts_with("Fattura") || l.starts_with("Nota") || l.starts_with("Parcella") || l.starts_with("Proforma") || l.starts_with("Autofattura") || l.starts_with("Integrazione") || l.starts_with("Vendita") || l.starts_with("Produzione");
    let l = if l == "DDT" { l.to_string() } else { l.to_lowercase() };
    format!("{} {}", if fem { "Nuova" } else { "Nuovo" }, l)
}
