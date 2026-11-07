//! Strumenti sulle righe del documento: calcolo inverso, copia di righe da un altro
//! documento, ultimi prezzi praticati, confezioni, sconto merce, documenti ricorrenti
//! e allegati.

use eframe::egui::{self, RichText};
use openfatt_core::calc;
use openfatt_core::model::*;
use openfatt_core::money::{fmt_it_trim, parse_dec};
use openfatt_store::{LastPrice, Recurrence, Store};
use rust_decimal::Decimal;

use super::theme::{self, icon};
use super::widgets::{self, eur, GRAY};
use super::Msgs;

/// Dialogo di copia righe: documento scelto e righe spuntate.
#[derive(Default)]
pub struct CopyState {
    pub search: String,
    pub doc: Option<Document>,
    pub checked: Vec<bool>,
}

#[derive(Default)]
pub struct Tools {
    /// riga, totale desiderato, totale IVA inclusa
    pub reverse_line: Option<(usize, String, bool)>,
    pub target_total: Option<String>,
    pub copy: Option<CopyState>,
    pub last_prices: Option<(usize, Vec<LastPrice>)>,
    pub packs: Option<(usize, String)>,
    pub percent: Option<(String, String)>,
    pub recurring: Option<Recurrence>,
    pub attachments: Option<Attachments>,
}

/// Esito di un dialogo che modifica le righe.
pub enum LineAction {
    None,
    Changed,
}

impl Tools {

    /// Mostra i dialoghi aperti. Restituisce Changed se il documento è stato modificato.
    pub fn show(&mut self, ctx: &egui::Context, store: &Store, msgs: &mut Msgs, doc: &mut Document) -> LineAction {
        let mut changed = false;
        changed |= self.reverse_line_dialog(ctx, doc, msgs);
        changed |= self.target_total_dialog(ctx, doc, msgs);
        changed |= self.copy_dialog(ctx, store, doc, msgs);
        changed |= self.last_prices_dialog(ctx, doc);
        changed |= self.packs_dialog(ctx, store, doc);
        changed |= self.percent_dialog(ctx, doc, msgs);
        self.recurring_dialog(ctx, store, msgs, doc);
        if let Some(a) = &mut self.attachments {
            if !a.show(ctx, store, msgs) {
                self.attachments = None;
            }
        }
        if changed {
            LineAction::Changed
        } else {
            LineAction::None
        }
    }

    fn reverse_line_dialog(&mut self, ctx: &egui::Context, doc: &mut Document, msgs: &mut Msgs) -> bool {
        let Some((i, target, with_vat)) = &mut self.reverse_line else { return false };
        let mut close = false;
        let mut changed = false;
        egui::Window::new("Calcola il prezzo dal totale").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            let Some(l) = doc.lines.get(*i).cloned() else {
                close = true;
                return;
            };
            ui.label(RichText::new(format!("Riga {}: {}", *i + 1, widgets::truncate(&l.description, 50))).color(GRAY));
            ui.label(format!("Quantità {} · sconti {} · IVA {}", fmt_it_trim(l.qty, 0, 4), if l.discounts.is_empty() { "nessuno" } else { &l.discounts }, l.vat.label()));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Totale della riga desiderato");
                ui.add(egui::TextEdit::singleline(target).desired_width(110.0).hint_text("es. 100,00")).request_focus();
            });
            ui.checkbox(with_vat, "Il totale comprende l'IVA");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let parsed = parse_dec(target);
                if theme::primary_enabled(ui, parsed.is_some(), icon::CALCULATOR, "Calcola il prezzo").clicked() {
                    if let Some(t) = parsed {
                        match calc::unit_price_for_total(t, l.qty, &l.discounts, l.vat.rate, *with_vat, doc.prices_include_vat) {
                            Some(p) => {
                                let l = &mut doc.lines[*i];
                                l.unit_price = p;
                                if l.product_id.is_some() {
                                    l.price_note = "Prezzo ricavato dal totale".into();
                                }
                                msgs.ok(format!("Prezzo unitario {}", fmt_it_trim(p, 2, 6)));
                                changed = true;
                                close = true;
                            }
                            None => msgs.err("Impossibile calcolare: quantità zero o sconto del 100%"),
                        }
                    }
                }
                if theme::button(ui, icon::X, "Annulla").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.reverse_line = None;
        }
        changed
    }

    fn target_total_dialog(&mut self, ctx: &egui::Context, doc: &mut Document, msgs: &mut Msgs) -> bool {
        let Some(target) = &mut self.target_total else { return false };
        let mut close = false;
        let mut changed = false;
        egui::Window::new("Porta il totale a...").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            let current = calc::compute(doc).map(|t| t.total).unwrap_or_default();
            ui.label(format!("Totale attuale IVA inclusa: € {}", eur(current)));
            ui.label(RichText::new("Si calcola lo sconto sul documento (o la maggiorazione, se il totale desiderato è più alto). Lo sconto documento attuale viene sostituito.").small().color(GRAY));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Totale desiderato IVA inclusa");
                ui.add(egui::TextEdit::singleline(target).desired_width(110.0)).request_focus();
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let parsed = parse_dec(target);
                if theme::primary_enabled(ui, parsed.is_some(), icon::CALCULATOR, "Applica").clicked() {
                    if let Some(t) = parsed {
                        match calc::discount_for_total(doc, t) {
                            Ok((d, got)) => {
                                doc.discount = if d.is_zero() { String::new() } else { fmt_it_trim(d, 0, 4).replace('.', "") };
                                if got == t {
                                    msgs.ok(format!("Sconto documento {}%: totale € {}", fmt_it_trim(d, 0, 4), eur(got)));
                                } else {
                                    msgs.warn(format!("Sconto {}%: il totale più vicino è € {} (arrotondamenti delle righe)", fmt_it_trim(d, 0, 4), eur(got)));
                                }
                                changed = true;
                                close = true;
                            }
                            Err(e) => msgs.err(e.0),
                        }
                    }
                }
                if theme::button(ui, icon::X, "Annulla").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.target_total = None;
        }
        changed
    }

    fn copy_dialog(&mut self, ctx: &egui::Context, store: &Store, doc: &mut Document, msgs: &mut Msgs) -> bool {
        let Some(cs) = &mut self.copy else { return false };
        let mut close = false;
        let mut changed = false;
        egui::Window::new("Copia righe da un altro documento").collapsible(false).default_width(720.0).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Cerca");
                ui.add(egui::TextEdit::singleline(&mut cs.search).desired_width(260.0).hint_text("cliente, fornitore o numero"));
            });
            ui.add_space(4.0);
            ui.columns(2, |cols| {
                egui::ScrollArea::vertical().id_salt("copy_docs").max_height(360.0).show(&mut cols[0], |ui| {
                    let f = openfatt_store::DocFilter { search: cs.search.clone(), ..Default::default() };
                    for d in store.documents(&f).unwrap_or_default().into_iter().filter(|d| d.id != doc.id).take(80) {
                        let sel = cs.doc.as_ref().map(|x| x.id) == Some(d.id);
                        let label = format!("{} {} · {} · {}", d.kind.label(), d.number_label, d.date.format("%d/%m/%Y"), widgets::truncate(&d.party_name, 28));
                        if ui.selectable_label(sel, label).clicked() {
                            if let Ok(full) = store.document(d.id) {
                                cs.checked = vec![true; full.lines.len()];
                                cs.doc = Some(full);
                            }
                        }
                    }
                });
                let ui = &mut cols[1];
                match &cs.doc {
                    None => {
                        ui.label(RichText::new("Scegliere un documento a sinistra").color(GRAY));
                    }
                    Some(src) => {
                        ui.horizontal(|ui| {
                            if ui.small_button("Tutte").clicked() {
                                cs.checked.iter_mut().for_each(|c| *c = true);
                            }
                            if ui.small_button("Nessuna").clicked() {
                                cs.checked.iter_mut().for_each(|c| *c = false);
                            }
                        });
                        egui::ScrollArea::vertical().id_salt("copy_lines").max_height(330.0).show(ui, |ui| {
                            for (i, l) in src.lines.iter().enumerate() {
                                let t = match l.kind {
                                    LineKind::Item => format!("{} x {}  € {}", fmt_it_trim(l.qty, 0, 3), widgets::truncate(&l.description, 40), eur(l.unit_price)),
                                    LineKind::Text => format!("(testo) {}", widgets::truncate(&l.description, 50)),
                                    LineKind::Subtotal => "(subtotale)".into(),
                                };
                                ui.checkbox(&mut cs.checked[i], t);
                            }
                        });
                    }
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let n = cs.checked.iter().filter(|c| **c).count();
                if theme::primary_enabled(ui, cs.doc.is_some() && n > 0, icon::COPY, &format!("Copia {} righe", n)).clicked() {
                    if let Some(src) = &cs.doc {
                        let purchase_to_sale = src.kind.is_purchase() != doc.kind.is_purchase();
                        for (l, _) in src.lines.iter().zip(cs.checked.iter()).filter(|(_, c)| **c) {
                            let mut c = openfatt_store::copy_line(l);
                            // da acquisto a vendita (o viceversa) il prezzo segue l'articolo
                            if purchase_to_sale {
                                if let Some(p) = l.product_id.and_then(|id| store.product(id).ok().flatten()) {
                                    c.unit_price = if doc.kind.is_purchase() { p.cost } else { p.price };
                                    c.discounts.clear();
                                    c.code = p.code.clone();
                                }
                            }
                            if src.prices_include_vat != doc.prices_include_vat && c.kind == LineKind::Item {
                                let m = Decimal::ONE + c.vat.rate / Decimal::ONE_HUNDRED;
                                c.unit_price = openfatt_core::money::round_dp(if doc.prices_include_vat { c.unit_price * m } else { c.unit_price / m }, 6);
                            }
                            doc.lines.push(c);
                        }
                        msgs.ok(format!("{} righe copiate da {}", n, src.title()));
                        changed = true;
                        close = true;
                    }
                }
                if theme::button(ui, icon::X, "Chiudi").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.copy = None;
        }
        changed
    }

    fn last_prices_dialog(&mut self, ctx: &egui::Context, doc: &mut Document) -> bool {
        let Some((i, list)) = &self.last_prices else { return false };
        let i = *i;
        let mut close = false;
        let mut changed = false;
        egui::Window::new("Ultimi prezzi praticati").collapsible(false).default_width(560.0).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            if list.is_empty() {
                ui.label(RichText::new("Nessun documento emesso con questo articolo per il soggetto.").color(GRAY));
            }
            egui::Grid::new("lp").striped(true).num_columns(6).spacing([12.0, 4.0]).show(ui, |ui| {
                for h in ["Documento", "Q.tà", "Prezzo", "Sconti", "Netto", ""] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for lp in list {
                    ui.label(&lp.title);
                    ui.label(fmt_it_trim(lp.qty, 0, 3));
                    ui.label(format!("{}{}", fmt_it_trim(lp.unit_price, 2, 6), if lp.prices_include_vat { " ivato" } else { "" }));
                    ui.label(if lp.discounts.is_empty() { "-".to_string() } else { lp.discounts.clone() });
                    ui.label(fmt_it_trim(lp.net, 2, 4));
                    if ui.button("Usa").clicked() {
                        if let Some(l) = doc.lines.get_mut(i) {
                            let m = Decimal::ONE + l.vat.rate / Decimal::ONE_HUNDRED;
                            l.unit_price = match (lp.prices_include_vat, doc.prices_include_vat) {
                                (true, false) => openfatt_core::money::round_dp(lp.unit_price / m, 6),
                                (false, true) => openfatt_core::money::round_dp(lp.unit_price * m, 6),
                                _ => lp.unit_price,
                            };
                            l.discounts = lp.discounts.clone();
                            l.price_note = format!("Ultimo prezzo ({})", lp.date.format("%d/%m/%Y"));
                            changed = true;
                        }
                        close = true;
                    }
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            if theme::button(ui, icon::X, "Chiudi").clicked() {
                close = true;
            }
        });
        if close {
            self.last_prices = None;
        }
        changed
    }

    fn packs_dialog(&mut self, ctx: &egui::Context, store: &Store, doc: &mut Document) -> bool {
        let Some((i, text)) = &mut self.packs else { return false };
        let i = *i;
        let mut close = false;
        let mut changed = false;
        let product = doc.lines.get(i).and_then(|l| l.product_id).and_then(|id| store.product(id).ok().flatten());
        let size = product.as_ref().map(|p| p.pack_size).unwrap_or_default();
        egui::Window::new("Quantità a confezioni").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            match &product {
                Some(p) if !size.is_zero() => {
                    let unit = if p.pack_unit.is_empty() { "confezioni".to_string() } else { p.pack_unit.clone() };
                    ui.label(format!("{}: {} {} per {}", p.description, fmt_it_trim(size, 0, 3), p.unit, unit));
                    ui.horizontal(|ui| {
                        ui.label(format!("Numero di {}", unit));
                        ui.add(egui::TextEdit::singleline(text).desired_width(80.0)).request_focus();
                        if let Some(n) = parse_dec(text) {
                            ui.label(RichText::new(format!("= {} {}", fmt_it_trim(n * size, 0, 3), p.unit)).color(GRAY));
                        }
                    });
                    ui.horizontal(|ui| {
                        if theme::primary_enabled(ui, parse_dec(text).is_some(), icon::CHECK, "Applica").clicked() {
                            if let (Some(n), Some(l)) = (parse_dec(text), doc.lines.get_mut(i)) {
                                l.packs = Some(n);
                                l.qty = n * size;
                                changed = true;
                            }
                            close = true;
                        }
                        if theme::button(ui, icon::X, "Annulla").clicked() {
                            close = true;
                        }
                    });
                }
                _ => {
                    ui.label("L'articolo non ha i pezzi per confezione: indicarli nella scheda articolo.");
                    if ui.button("Chiudi").clicked() {
                        close = true;
                    }
                }
            }
        });
        if close {
            self.packs = None;
        }
        changed
    }

    fn percent_dialog(&mut self, ctx: &egui::Context, doc: &mut Document, msgs: &mut Msgs) -> bool {
        let Some((desc, pct)) = &mut self.percent else { return false };
        let mut close = false;
        let mut changed = false;
        egui::Window::new("Riga calcolata").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label(RichText::new("L'importo è una percentuale della somma delle righe precedenti e si aggiorna da solo (spese di trasporto, imballo, contributi...).").small().color(GRAY));
            egui::Grid::new("pct").num_columns(2).show(ui, |ui| {
                ui.label("Descrizione");
                ui.add(egui::TextEdit::singleline(desc).desired_width(260.0));
                ui.end_row();
                ui.label("Percentuale");
                ui.add(egui::TextEdit::singleline(pct).desired_width(70.0).hint_text("es. 3"));
                ui.end_row();
            });
            ui.horizontal(|ui| {
                let p = parse_dec(pct);
                if theme::primary_enabled(ui, p.is_some() && !desc.trim().is_empty(), icon::PLUS, "Aggiungi").clicked() {
                    let vat = doc.lines.iter().rev().find(|l| l.kind == LineKind::Item).map(|l| l.vat.clone()).unwrap_or_default();
                    doc.lines.push(DocLine { description: desc.trim().to_string(), calc_percent: p, vat, qty: Decimal::ONE, ..Default::default() });
                    msgs.ok("Riga calcolata aggiunta");
                    changed = true;
                    close = true;
                }
                if theme::button(ui, icon::X, "Annulla").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.percent = None;
        }
        changed
    }

    fn recurring_dialog(&mut self, ctx: &egui::Context, store: &Store, msgs: &mut Msgs, doc: &Document) {
        let Some(r) = &mut self.recurring else { return };
        let mut close = false;
        egui::Window::new("Documento ricorrente").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label(RichText::new("Questo documento resta in bozza come modello: alle date previste se ne crea una copia. Nelle descrizioni si possono usare {mese}, {anno} e {periodo}.").small().color(GRAY));
            ui.add_space(4.0);
            egui::Grid::new("rec").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Descrizione");
                ui.add(egui::TextEdit::singleline(&mut r.description).desired_width(260.0));
                ui.end_row();
                ui.label("Ogni");
                egui::ComboBox::from_id_salt("rec_every").selected_text(every_label(r.every_months)).show_ui(ui, |ui| {
                    for m in [1, 2, 3, 4, 6, 12] {
                        ui.selectable_value(&mut r.every_months, m, every_label(m));
                    }
                });
                ui.end_row();
                ui.label("Prossimo documento");
                widgets::date_field(ui, "rec_next", &mut r.next_date);
                ui.end_row();
                ui.label("Fino al");
                widgets::opt_date_field(ui, "rec_end", &mut r.end_date);
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut r.issue, "Emetti subito i documenti generati");
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut r.active, "Attivo");
                ui.end_row();
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::primary_enabled(ui, doc.id != 0 && doc.status == DocStatus::Draft, icon::CHECK, "Salva").clicked() {
                    r.template_id = doc.id;
                    if msgs.check(store.save_recurrence(r), "Documento ricorrente salvato").is_some() {
                        close = true;
                    }
                }
                if r.id != 0 && theme::button(ui, icon::TRASH, "Elimina la ricorrenza").clicked() {
                    if msgs.check(store.delete_recurrence(r.id), "Ricorrenza eliminata").is_some() {
                        close = true;
                    }
                }
                if theme::button(ui, icon::X, "Chiudi").clicked() {
                    close = true;
                }
            });
            if doc.id == 0 || doc.status != DocStatus::Draft {
                ui.label(RichText::new("Salvare il documento in bozza prima di renderlo ricorrente.").color(super::widgets::ORANGE));
            }
        });
        if close {
            self.recurring = None;
        }
    }
}

pub fn every_label(m: u32) -> String {
    match m {
        1 => "mese".into(),
        2 => "2 mesi (bimestrale)".into(),
        3 => "3 mesi (trimestrale)".into(),
        6 => "6 mesi (semestrale)".into(),
        12 => "anno".into(),
        n => format!("{} mesi", n),
    }
}

/// Finestra degli allegati di un documento, soggetto o articolo.
pub struct Attachments {
    pub entity: String,
    pub id: i64,
    pub title: String,
}

impl Attachments {
    pub fn new(entity: &str, id: i64, title: impl Into<String>) -> Self {
        Attachments { entity: entity.into(), id, title: title.into() }
    }

    /// Restituisce false quando la finestra va chiusa.
    pub fn show(&mut self, ctx: &egui::Context, store: &Store, msgs: &mut Msgs) -> bool {
        let mut open = true;
        egui::Window::new(format!("Allegati - {}", self.title)).collapsible(false).default_width(520.0).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            let list = store.attachments(&self.entity, self.id).unwrap_or_default();
            if list.is_empty() {
                ui.label(RichText::new("Nessun allegato. Si possono allegare PDF, immagini e documenti acquisiti con lo scanner.").color(GRAY));
            }
            egui::Grid::new("att").striped(true).num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
                for a in &list {
                    ui.label(widgets::truncate(&a.name, 40));
                    ui.label(RichText::new(format!("{} kB", (a.size + 1023) / 1024)).color(GRAY));
                    if ui.small_button("Apri").clicked() {
                        if let Ok((name, data)) = store.attachment_content(a.id) {
                            if let Err(e) = crate::services::open_bytes(&name, &data) {
                                msgs.err(e);
                            }
                        }
                    }
                    if ui.small_button(icon::TRASH).on_hover_text("Elimina l'allegato").clicked() {
                        msgs.check(store.delete_attachment(a.id), "Allegato eliminato");
                    }
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::primary(ui, icon::PAPERCLIP, "Aggiungi file...").clicked() {
                    if let Some(files) = rfd::FileDialog::new().add_filter("Documenti e immagini", &["pdf", "png", "jpg", "jpeg", "tif", "tiff", "xml", "p7m", "txt", "csv", "docx", "xlsx"]).pick_files() {
                        for f in files {
                            match std::fs::read(&f) {
                                Ok(data) => {
                                    let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "allegato".into());
                                    msgs.check(store.add_attachment(&self.entity, self.id, &name, &data), &format!("{} allegato", name));
                                }
                                Err(e) => msgs.err(e.to_string()),
                            }
                        }
                    }
                }
                if theme::button(ui, icon::SCAN, "Da scanner...").on_hover_text("Apre l'acquisizione immagini di Windows; poi si allega il file salvato").clicked() {
                    // Acquisizione fax e scanner di Windows: il file acquisito si allega con "Aggiungi file"
                    let _ = std::process::Command::new("wfs.exe").spawn().or_else(|_| std::process::Command::new("mspaint.exe").spawn());
                    msgs.ok("Acquisire il documento e salvarlo, poi allegarlo con \"Aggiungi file\"");
                }
                if theme::button(ui, icon::X, "Chiudi").clicked() {
                    open = false;
                }
            });
        });
        open
    }
}
