//! Editor del documento: testata, righe, fiscalità, trasporto, riferimenti, totali.

use eframe::egui::{self, RichText};
use openfatt_core::calc;
use openfatt_core::codes;
use openfatt_core::model::*;
use openfatt_core::money::fmt_it_trim;
use rust_decimal::Decimal;

use super::theme::{self, icon, Tone};
use super::widgets::{self, eur, GRAY, ORANGE, RED};
use super::{Ctx, DocGroup, Nav, Screen};
use crate::services;

pub struct Editor {
    pub doc: Document,
    pub dirty: bool,
    lock: Option<String>,
    vats: Vec<VatCode>,
    payments: Vec<PaymentMethod>,
    quick_code: String,
    manual_number: String,
    confirm_close: bool,
    fe_status: Option<String>,
    fulfillment: (bool, bool),
    pub tools: super::linetools::Tools,
    intent_cache: Option<(u64, String, Option<openfatt_store::IntentUsage>)>,
    /// Scadenze già in una distinta: il documento resta bloccato ma si può emettere
    in_batch: bool,
    tab: DocTab,
    cols: LineCols,
}

/// Colonne facoltative delle righe, ricordate nell'archivio.
#[derive(Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct LineCols {
    hide_cost: bool,
    hide_net: bool,
    hide_margin: bool,
}

/// Schede della finestra del documento.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DocTab {
    Lines,
    Payment,
    Transport,
    Notes,
    Addresses,
    FeProps,
    Options,
}

fn group_of(k: DocKind) -> DocGroup {
    match k {
        DocKind::Quote => DocGroup::Quotes,
        DocKind::CustomerOrder => DocGroup::Orders,
        DocKind::DeliveryNote => DocGroup::DeliveryNotes,
        DocKind::Proforma => DocGroup::Proforma,
        DocKind::SelfInvoice => DocGroup::SelfInvoices,
        DocKind::SupplierQuote => DocGroup::SupplierQuotes,
        DocKind::SupplierOrder => DocGroup::SupplierOrders,
        DocKind::GoodsReceipt => DocGroup::GoodsReceipts,
        DocKind::PurchaseInvoice | DocKind::PurchaseCreditNote => DocGroup::PurchaseInvoices,
        DocKind::Production => DocGroup::Production,
        _ => DocGroup::Invoices,
    }
}

impl Editor {
    pub fn new(store: &openfatt_store::Store, doc: Document) -> Self {
        let lock = if doc.id != 0 { store.document_lock_reason(doc.id).ok().flatten() } else { None };
        let fe_status = if doc.id != 0 { store.einvoice_for_document(doc.id).ok().flatten().map(|e| e.status.label().to_string()) } else { None };
        let fulfillment = if doc.id != 0 { store.fulfillment_state(&doc).unwrap_or((false, false)) } else { (false, false) };
        let in_batch = doc.id != 0 && store.document_in_batch(doc.id).unwrap_or(false);
        Editor {
            manual_number: doc.number.map(|n| n.to_string()).unwrap_or_default(),
            doc,
            dirty: false,
            lock,
            vats: store.vat_codes().unwrap_or_default(),
            payments: store.payment_methods().unwrap_or_default(),
            quick_code: String::new(),
            confirm_close: false,
            fe_status,
            fulfillment,
            tools: Default::default(),
            intent_cache: None,
            in_batch,
            tab: DocTab::Lines,
            cols: store.get_setting("doc_line_columns").unwrap_or_default(),
        }
    }

    /// Apre il documento in sola lettura (altra postazione o permessi).
    pub fn set_readonly(&mut self, reason: &str) {
        self.lock = Some(reason.to_string());
    }

    pub fn back_screen(&self) -> Screen {
        if self.doc.kind == DocKind::RetailSale {
            return Screen::Counter;
        }
        if self.doc.kind == DocKind::StockTransfer {
            return Screen::Stock;
        }
        if self.doc.kind == DocKind::Production {
            return Screen::Documents(DocGroup::Production);
        }
        Screen::Documents(group_of(self.doc.kind))
    }

    fn default_vat(&self) -> VatRef {
        if let Some(id) = self.doc.party.default_vat_id {
            if let Some(v) = self.vats.iter().find(|v| v.id == id) {
                return self.pa_vat(VatRef::from_code(v));
            }
        }
        let base = self.vats.iter().find(|v| v.code == "22").map(VatRef::from_code).unwrap_or(VatRef { vat_id: None, rate: Decimal::from(22), natura: None, legal_ref: String::new(), split: false });
        self.pa_vat(base)
    }

    /// Fatture alla Pubblica Amministrazione con scissione: si usa l'aliquota "sp" corrispondente.
    fn pa_vat(&self, v: VatRef) -> VatRef {
        if !(self.doc.party.is_pa && self.doc.kind.is_fiscal() && self.doc.vat_liability == "S") || v.rate.is_zero() || v.split {
            return v;
        }
        self.vats.iter().find(|x| x.active && x.split_payment && x.rate == v.rate).map(VatRef::from_code).unwrap_or(v)
    }

    /// Allinea le aliquote delle righe al soggetto (con o senza scissione dei pagamenti).
    fn sync_split_lines(&mut self) {
        let want = self.doc.party.is_pa && self.doc.kind.is_fiscal() && self.doc.vat_liability == "S";
        for i in 0..self.doc.lines.len() {
            let v = self.doc.lines[i].vat.clone();
            if want {
                self.doc.lines[i].vat = self.pa_vat(v);
            } else if v.split {
                if let Some(x) = self.vats.iter().find(|x| x.active && !x.split_payment && x.rate == v.rate && x.natura.is_none()) {
                    self.doc.lines[i].vat = VatRef::from_code(x);
                }
            }
        }
    }

    /// Prezzo (con eventuale conversione a IVA inclusa), sconti e provenienza per l'articolo.
    fn priced(&self, store: &openfatt_store::Store, p: &Product, qty: Decimal, vat_rate: Decimal) -> (Decimal, String, String) {
        // il listino scelto nel documento vale al posto di quello del cliente
        let with_list = self.doc.price_list_id.filter(|_| self.doc.kind.is_sale()).map(|l| Party { price_list_id: if l == 0 { None } else { Some(l) }, ..self.doc.party.clone() });
        let party = with_list.as_ref().or(if self.doc.party_id.is_some() { Some(&self.doc.party) } else { None });
        let q = store.quote(party, p, qty, self.doc.date, self.doc.kind.is_purchase()).unwrap_or_default();
        let mut price = q.price;
        if self.doc.prices_include_vat {
            price = openfatt_core::money::round_dp(price * (Decimal::ONE + vat_rate / Decimal::ONE_HUNDRED), 4);
        }
        (openfatt_core::money::round_dp(price, 6), q.discounts, q.source)
    }

    /// Ricalcola prezzi e sconti di tutte le righe di articolo secondo listini e condizioni.
    fn reprice_all(&mut self, store: &openfatt_store::Store) -> usize {
        let mut n = 0;
        for i in 0..self.doc.lines.len() {
            let l = &self.doc.lines[i];
            let Some(pid) = l.product_id else { continue };
            let Some(p) = store.product(pid).ok().flatten() else { continue };
            let (price, disc, note) = self.priced(store, &p, l.qty, l.vat.rate);
            let code = self.supplier_code_of(&p);
            let l = &mut self.doc.lines[i];
            l.unit_price = price;
            l.discounts = disc;
            l.price_note = note;
            l.code = code;
            n += 1;
        }
        n
    }

    /// Passa da prezzi netti a prezzi IVA inclusa (o viceversa) convertendo le righe.
    fn set_price_mode(&mut self, include_vat: bool) {
        if self.doc.prices_include_vat == include_vat {
            return;
        }
        self.doc.prices_include_vat = include_vat;
        for l in self.doc.lines.iter_mut().filter(|l| l.kind == LineKind::Item && l.calc_percent.is_none()) {
            let k = Decimal::ONE + l.vat.rate / Decimal::ONE_HUNDRED;
            l.unit_price = openfatt_core::money::round_dp(if include_vat { l.unit_price * k } else { l.unit_price / k }, 6);
        }
        self.dirty = true;
    }

    /// Ricavo e costo delle righe con prezzo di acquisto, e quante righe ne sono prive.
    fn margin_totals(&self, t: &calc::Totals) -> (Decimal, Decimal, usize) {
        let (mut rev, mut cost, mut missing) = (Decimal::ZERO, Decimal::ZERO, 0);
        for lc in &t.lines {
            let Some(l) = lc.index.and_then(|i| self.doc.lines.get(i)).filter(|l| l.kind == LineKind::Item && l.calc_percent.is_none()) else { continue };
            if l.unit_cost.is_zero() {
                if !lc.total.is_zero() {
                    missing += 1;
                }
                continue;
            }
            rev += lc.total;
            cost += l.unit_cost * l.qty;
        }
        (rev, cost, missing)
    }

    /// Codice da mostrare in riga: negli acquisti quello del fornitore del documento.
    fn supplier_code_of(&self, p: &Product) -> String {
        if self.doc.kind.is_purchase() {
            if let Some((c, _)) = self.doc.party_id.and_then(|id| p.supplier_terms(id)).filter(|(c, _)| !c.trim().is_empty()) {
                return c;
            }
        }
        p.code.clone()
    }

    fn line_from_product(&self, store: &openfatt_store::Store, p: &Product) -> DocLine {
        let vat = if self.doc.party.default_vat_id.is_some() {
            self.default_vat()
        } else {
            p.vat_id.and_then(|id| self.vats.iter().find(|v| v.id == id)).map(|v| self.pa_vat(VatRef::from_code(v))).unwrap_or_else(|| self.default_vat())
        };
        let (price, discounts, price_note) = self.priced(store, p, Decimal::ONE, vat.rate);
        DocLine {
            product_id: Some(p.id),
            code: self.supplier_code_of(p),
            description: p.description.clone(),
            qty: Decimal::ONE,
            unit: p.unit.clone(),
            unit_price: price,
            discounts,
            price_note,
            vat,
            withholding: !self.doc.withholdings.is_empty() && p.is_service,
            pension_fund: self.doc.pension_fund.is_some(),
            unit_cost: p.cost,
            ..Default::default()
        }
    }

    /// Righe per un articolo: un gruppo (kit) si espande nei componenti; il contributo
    /// ambientale dell'articolo passa sulla riga.
    pub fn lines_for_product(&self, store: &openfatt_store::Store, p: &Product) -> Vec<DocLine> {
        let mut first = self.line_from_product(store, p);
        first.eco_fee = p.eco_fee;
        first.eco_label = p.eco_label.clone();
        if p.kind != ProductKind::Kit || p.components.is_empty() || self.doc.kind.is_internal() {
            return vec![first];
        }
        let comps: Vec<(Product, Decimal)> = p.components.iter().filter_map(|c| store.product(c.product_id).ok().flatten().map(|x| (x, c.qty))).collect();
        let mut out = Vec::new();
        if p.price.is_zero() {
            // gruppo senza prezzo proprio: intestazione e componenti ai loro prezzi
            out.push(DocLine { kind: LineKind::Text, description: p.description.clone(), ..Default::default() });
            for (cp, q) in comps {
                let mut l = self.line_from_product(store, &cp);
                l.qty = q;
                l.eco_fee = cp.eco_fee;
                l.eco_label = cp.eco_label.clone();
                l.description = format!("  {}", l.description);
                out.push(l);
            }
        } else {
            // gruppo con prezzo: riga del gruppo e componenti a prezzo zero (scaricano il magazzino)
            first.product_id = if p.manage_stock { Some(p.id) } else { None };
            out.push(first.clone());
            for (cp, q) in comps {
                let mut l = self.line_from_product(store, &cp);
                l.qty = q;
                l.unit_price = Decimal::ZERO;
                l.discounts.clear();
                l.price_note = format!("Componente di {}", p.code);
                l.description = format!("  - {}", l.description);
                l.vat = first.vat.clone();
                out.push(l);
            }
        }
        out
    }

    fn set_party(&mut self, store: &openfatt_store::Store, msgs: &mut super::Msgs, p: Party) {
        let changed_party = self.doc.party_id != Some(p.id);
        self.doc.party_id = Some(p.id);
        if self.doc.payment_id.is_none() {
            self.doc.payment_id = p.default_payment_id;
        }
        if p.is_pa && self.doc.kind.is_fiscal() && self.doc.vat_liability == "I" {
            self.doc.vat_liability = "S".into();
        } else if !p.is_pa && self.doc.vat_liability == "S" {
            self.doc.vat_liability = "I".into();
        }
        if self.doc.agent_id.is_none() && self.doc.kind.is_sale() {
            self.doc.agent_id = p.agent_id;
        }
        // dichiarazione d'intento valida alla data del documento
        if self.doc.intent_protocol.is_empty() && self.doc.kind.is_fiscal() {
            if let Some(i) = p.intents.iter().find(|i| i.from.map(|f| f <= self.doc.date).unwrap_or(true) && i.to.map(|t| t >= self.doc.date).unwrap_or(true) && (i.year == 0 || i.year == chrono::Datelike::year(&self.doc.date))) {
                self.doc.intent_protocol = i.protocol.clone();
            }
        }
        self.doc.party = p;
        self.dirty = true;
        if changed_party {
            self.doc.price_list_id = None;
        }
        self.sync_split_lines();
        // in bozza i prezzi seguono listino e condizioni del nuovo soggetto
        if changed_party && self.doc.status == DocStatus::Draft && self.doc.lines.iter().any(|l| l.product_id.is_some()) {
            let n = self.reprice_all(store);
            msgs.ok(format!("Prezzi di {} righe aggiornati secondo listino e condizioni di {}", n, self.doc.party.display_name()));
        }
    }

    fn save(&mut self, cx: &mut Ctx) -> bool {
        if let Some(pid) = self.doc.payment_id {
            self.doc.payment = self.payments.iter().find(|p| p.id == pid).cloned();
        } else {
            self.doc.payment = None;
        }
        if self.doc.status == DocStatus::Draft {
            self.doc.number = self.manual_number.trim().parse().ok();
        }
        let was_issued = self.doc.status == DocStatus::Issued;
        match cx.store.save_document(&self.doc) {
            Ok(d) => {
                self.manual_number = d.number.map(|n| n.to_string()).unwrap_or_default();
                self.doc = d;
                self.dirty = false;
                if was_issued {
                    if let Ok(Some(e)) = cx.store.einvoice_for_document(self.doc.id) {
                        if e.status == openfatt_store::EInvoiceStatus::ToSend {
                            let _ = cx.store.delete_einvoice(e.id);
                            cx.msgs.warn("Il documento è cambiato: la fattura elettronica non inviata è stata rimossa, rigenerarla");
                            self.fe_status = None;
                        }
                    }
                }
                true
            }
            Err(e) => {
                cx.msgs.err(e.to_string());
                false
            }
        }
    }

    fn title_bar(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let locked = self.lock.is_some();
        let save_shortcut = ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::S));
        if save_shortcut && !locked && self.doc.status != DocStatus::Cancelled && self.save(cx) {
            cx.msgs.ok("Documento salvato");
        }
        let title = if self.doc.number.is_some() { format!("{} {}", self.doc.kind.label(), self.doc.number_label()) } else { format!("{} (nuovo)", self.doc.kind.label()) };
        let mut sub = String::new();
        if let Some(l) = &self.lock {
            sub = format!("sola lettura: {}", l);
        }
        let job = theme::subline(&[(sub.as_str(), false)]);
        let status = self.doc.status;
        let dirty = self.dirty;
        let ff = self.fulfillment;
        theme::title_row(ui, &[], &title, Some(job), |ui| {
            let tone = match status {
                DocStatus::Draft => Tone::Warning,
                DocStatus::Issued => Tone::Positive,
                DocStatus::Cancelled => Tone::Negative,
            };
            theme::pill(ui, status.label(), tone);
            if dirty {
                theme::pill(ui, "Modifiche non salvate", Tone::Warning);
            }
            if ff.0 {
                theme::pill(ui, "Evaso", Tone::Positive);
            } else if ff.1 {
                theme::pill(ui, "Evaso in parte", Tone::Warning);
            }
        });
    }

    fn issue(&mut self, cx: &mut Ctx) {
        let frozen = self.in_batch || self.doc.due_dates.iter().any(|d| !d.paid.is_zero() || !d.writeoff.is_zero());
        if !frozen && !self.save(cx) {
            return;
        }
        match cx.store.issue_document(self.doc.id) {
            Ok(d) => {
                cx.msgs.ok(format!("{} emesso con numero {}", d.kind.label(), d.number_label()));
                self.manual_number = d.number.map(|n| n.to_string()).unwrap_or_default();
                self.doc = d;
            }
            Err(e) => cx.msgs.err(e.to_string()),
        }
    }

    /// Pulsanti in fondo alla finestra del documento.
    fn action_bar(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let locked = self.lock.is_some();
        let group = group_of(self.doc.kind);
        let purchase = DocGroup::PURCHASES.contains(&group);
        let can_fe = self.doc.kind.is_fiscal() && self.doc.status == DocStatus::Issued && self.fe_status.is_none() && !purchase && !self.doc.legacy_fe;
        let can_save = !locked && self.doc.status != DocStatus::Cancelled;
        let saved = self.doc.id != 0 && !self.dirty;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            if theme::ghost(ui, true, icon::PRINTER, "Stampa").on_hover_text("Anteprima di stampa in PDF").clicked() {
                self.open_pdf(cx);
            }
            egui::menu::menu_custom_button(ui, egui::Button::new(format!("{}  Etichette", icon::TAG)).frame(false).min_size(egui::vec2(0.0, 27.0)), |ui| {
                if ui.button("Etichette degli articoli...").clicked() {
                    ui.close_menu();
                    cx.nav.push(Nav::Labels(crate::printing::document_items(cx.store, &self.doc)));
                }
                if crate::printing::is_shipping_doc(self.doc.kind) && self.doc.id != 0 && ui.button("Etichette di spedizione...").clicked() {
                    ui.close_menu();
                    cx.nav.push(Nav::Labels(vec![crate::printing::shipping_item(cx.store, &self.doc)]));
                }
            });
            if self.doc.id != 0 {
                let n = cx.store.attachments("document", self.doc.id).map(|a| a.len()).unwrap_or(0);
                if theme::ghost(ui, true, icon::PAPERCLIP, &if n > 0 { format!("Allegati ({})", n) } else { "Allegati...".to_string() }).clicked() {
                    self.tools.attachments = Some(super::linetools::Attachments::new("document", self.doc.id, self.doc.title()));
                }
            }
            if !purchase && self.doc.kind.is_fiscal() && !locked && theme::ghost(ui, true, icon::PERCENT, "Rit. acconto").clicked() {
                self.tab = DocTab::Payment;
            }
            if can_save && theme::ghost(ui, true, icon::COPY, "Includi doc.").on_hover_text("Copia le righe da un altro documento").clicked() {
                self.tools.copy = Some(Default::default());
            }
            if saved && self.doc.status != DocStatus::Cancelled {
                let targets = self.doc.kind.transform_targets();
                if self.doc.kind.has_order_state() && !self.fulfillment.0 {
                    egui::menu::menu_custom_button(ui, egui::Button::new(theme::bold(format!("{}  Concludi ordine", icon::CHECK_CIRCLE))).frame(false).min_size(egui::vec2(0.0, 27.0)), |ui| {
                        for t in targets {
                            if ui.button(t.label()).clicked() {
                                ui.close_menu();
                                if let Some(d) = cx.msgs.check(cx.store.transform(&[self.doc.id], *t), "") {
                                    cx.msgs.ok(format!("{} creato", t.label()));
                                    cx.nav.push(Nav::OpenDocument(d.id));
                                }
                            }
                        }
                        ui.separator();
                        if ui.button("Segna come concluso, senza altri documenti").clicked() {
                            ui.close_menu();
                            self.doc.order_status = OrderStatus::Concluded;
                            self.dirty = true;
                        }
                    });
                } else if !self.doc.kind.has_order_state() {
                    for t in targets {
                        let label = if *t == DocKind::SelfInvoice { "Autofattura / integrazione" } else { t.label() };
                        if theme::ghost(ui, true, icon::ARROW_RIGHT, label).clicked() {
                            if let Some(d) = cx.msgs.check(cx.store.transform(&[self.doc.id], *t), "") {
                                cx.msgs.ok(format!("{} creato", t.label()));
                                cx.nav.push(Nav::OpenDocument(d.id));
                            }
                        }
                    }
                }
            }
            if can_fe && !self.dirty && theme::primary(ui, icon::FILE_ARROW_UP, "Fattura elettr.").on_hover_text("Genera la fattura elettronica").clicked() {
                self.generate_fe(cx);
            }
            egui::menu::menu_custom_button(ui, egui::Button::new(format!("{}  Altro", icon::DOTS_THREE)).frame(false).min_size(egui::vec2(0.0, 27.0)), |ui| {
                if !purchase && self.doc.id != 0 && !self.dirty && self.doc.status != DocStatus::Cancelled && ui.button(format!("{}  Invia per email...", icon::ENVELOPE_SIMPLE)).clicked() {
                    ui.close_menu();
                    match crate::mailer::for_document(cx.store, &self.doc) {
                        Ok(c) => cx.nav.push(Nav::Mail(vec![c])),
                        Err(e) => cx.msgs.err(e),
                    }
                }
                if ui.button(format!("{}  Salva PDF in una cartella...", icon::DOWNLOAD_SIMPLE)).clicked() {
                    ui.close_menu();
                    if let Some(p) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(format!("{} {}.pdf", self.doc.kind.label(), self.doc.number_label().replace('/', "-"))).save_file() {
                        match services::document_pdf(cx.store, &self.doc).and_then(|pdf| std::fs::write(&p, pdf).map_err(|e| e.to_string())) {
                            Ok(_) => cx.msgs.ok(format!("PDF salvato in {}", p.display())),
                            Err(e) => cx.msgs.err(e),
                        }
                    }
                }
                if self.doc.kind.is_sale() && !self.doc.kind.is_internal() && self.doc.kind != DocKind::RetailSale && self.doc.status == DocStatus::Draft {
                    let existing = cx.store.recurrences().unwrap_or_default().into_iter().find(|r| r.template_id == self.doc.id && self.doc.id != 0);
                    let label = if existing.is_some() { "Modifica la ricorrenza..." } else { "Rendi ricorrente..." };
                    if ui.button(format!("{}  {}", icon::REPEAT, label)).clicked() {
                        ui.close_menu();
                        self.tools.recurring = Some(existing.unwrap_or_else(|| openfatt_store::Recurrence { template_id: self.doc.id, description: self.doc.party.display_name(), next_date: openfatt_store::add_months(self.doc.date, 1), ..Default::default() }));
                    }
                }
                if self.doc.kind.is_purchase() && self.doc.id != 0 && !self.dirty && ui.button(format!("{}  Carica gli articoli in anagrafica", icon::PACKAGE)).clicked() {
                    ui.close_menu();
                    if let Some(r) = cx.msgs.check(cx.store.import_products_from_purchase(self.doc.id, "", true), "") {
                        cx.msgs.ok(format!("Articoli: {} nuovi, {} aggiornati, {} righe saltate", r.created, r.updated, r.skipped));
                        if let Ok(d) = cx.store.document(self.doc.id) {
                            self.doc = d;
                        }
                    }
                }
                if ui.button(format!("{}  Personalizza la stampa...", icon::PAINT_BRUSH)).clicked() {
                    ui.close_menu();
                    cx.nav.push(Nav::PrintSettings);
                }
                if self.doc.status == DocStatus::Issued && !locked && ui.button(format!("{}  Riporta in bozza", icon::ARROW_COUNTER_CLOCKWISE)).clicked() {
                    ui.close_menu();
                    if let Some(d) = cx.msgs.check(cx.store.revert_to_draft(self.doc.id), "Documento riportato in bozza") {
                        self.doc = d;
                    }
                }
                if self.doc.status != DocStatus::Draft && self.dirty && ui.button(format!("{}  Annulla le modifiche", icon::ARROW_COUNTER_CLOCKWISE)).clicked() {
                    ui.close_menu();
                    cx.nav.push(Nav::OpenDocument(self.doc.id));
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::ghost(ui, true, icon::X, "Chiudi").clicked() {
                    if self.dirty {
                        self.confirm_close = true;
                    } else {
                        cx.nav.push(Nav::CloseEditor);
                    }
                }
                if self.doc.status == DocStatus::Draft {
                    let frozen = self.in_batch || self.doc.due_dates.iter().any(|d| !d.paid.is_zero() || !d.writeoff.is_zero());
                    if theme::primary_enabled(ui, can_save || frozen, icon::CHECK, "Emetti").on_hover_text("Conferma il documento: muove il magazzino e permette la fattura elettronica").clicked() {
                        self.issue(cx);
                    }
                    if theme::button_enabled(ui, can_save, icon::FLOPPY_DISK, "Salva").on_hover_text("Ctrl+S: registra numero, dati e scadenze").clicked() && self.save(cx) {
                        cx.msgs.ok("Documento salvato");
                    }
                } else if self.dirty && theme::primary_enabled(ui, can_save, icon::FLOPPY_DISK, "Salva").on_hover_text("Ctrl+S").clicked() && self.save(cx) {
                    cx.msgs.ok("Documento salvato");
                }
            });
        });
    }

    fn open_pdf(&self, cx: &mut Ctx) {
        if let Err(e) = services::document_pdf(cx.store, &self.doc).and_then(|pdf| services::open_bytes(&format!("{}_{}.pdf", self.doc.kind.code(), self.doc.number_label().replace(['/', '(', ')'], "-")), &pdf)) {
            cx.msgs.err(e);
        }
    }

    fn generate_fe(&mut self, cx: &mut Ctx) {
        if self.dirty && !self.save(cx) {
            return;
        }
        match services::create_einvoice(cx.store, self.doc.id) {
            Ok((e, issues)) => {
                self.fe_status = Some(e.status.label().to_string());
                let warns: Vec<String> = issues.iter().filter(|i| i.severity == openfatt_fatturapa::Severity::Warning).map(|i| i.to_string()).collect();
                if warns.is_empty() {
                    cx.msgs.ok(format!("Fattura elettronica {} pronta per l'invio", e.file_name));
                } else {
                    cx.msgs.info_dialog("Fattura elettronica generata con avvisi", format!("{}\n\n{}", e.file_name, warns.join("\n")));
                }
                self.lock = cx.store.document_lock_reason(self.doc.id).ok().flatten();
            }
            Err(e) => cx.msgs.err(e),
        }
    }

    /// Percorso della fattura elettronica: emessa, XML, invio, esito, incasso.
    fn sdi_track(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        if !(self.doc.kind.is_fiscal() && self.doc.kind.is_sale() && self.doc.status == DocStatus::Issued && self.doc.id != 0) {
            return;
        }
        let e = cx.store.einvoice_for_document(self.doc.id).ok().flatten();
        let code = e.as_ref().map(|e| e.status.code());
        let (done, _, ko, text) = theme::sdi_progress(code);
        let paid = !self.doc.due_dates.is_empty() && self.doc.due_dates.iter().all(|d| d.is_paid());
        let next_due = self.doc.due_dates.iter().find(|d| !d.is_paid()).map(|d| d.date.format("%d/%m/%Y").to_string());
        let sent = e.as_ref().and_then(|e| e.sent_at.clone()).unwrap_or_default();
        // (titolo, dettaglio, stato: 0 da fare, 1 fatto, 2 corrente, 3 errore)
        let steps: [(&str, String, u8); 5] = [
            ("Emessa", self.doc.date.format("%d/%m/%Y").to_string(), 1),
            ("XML generato", e.as_ref().map(|e| e.file_name.clone()).unwrap_or_else(|| "da generare".into()), if done >= 1 { 1 } else { 2 }),
            ("Inviata allo SdI", if done >= 2 { sent.chars().take(16).collect() } else { "via PEC".into() }, if done >= 2 { 1 } else if done == 1 { 2 } else { 0 }),
            (if ko { "Esito" } else { "Consegnata" }, if done >= 2 { text.to_string() } else { "ricevuta dello SdI".into() }, if ko { 3 } else if done >= 3 { 1 } else if done == 2 { 2 } else { 0 }),
            (if self.doc.kind.is_credit() { "Chiusa" } else if paid { "Incassata" } else { "Da incassare" }, if paid { "tutte le rate".into() } else { next_due.map(|d| format!("prossima rata {}", d)).unwrap_or_else(|| "-".into()) }, if paid { 1 } else if done >= 3 { 2 } else { 0 }),
        ];
        theme::card(
            ui,
            "Fattura elettronica",
            |ui| {
                if ko {
                    ui.label(RichText::new("correggere il documento e generare di nuovo l'XML").size(12.0).color(theme::NEGATIVE));
                }
            },
            |ui| {
                let w = ui.available_width();
                let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 58.0), egui::Sense::hover());
                let p = ui.painter();
                let n = steps.len() as f32;
                let col = w / n;
                for (i, (t, d, s)) in steps.iter().enumerate() {
                    let x = rect.left() + col * i as f32;
                    let y = rect.top() + 7.0;
                    let line_c = if *s == 1 { theme::PRIMARY } else { theme::TRACK };
                    p.hline(x..=x + col - 4.0, y, egui::Stroke::new(2.0_f32, line_c));
                    let (fill, stroke) = match s {
                        1 => (theme::PRIMARY, theme::PRIMARY),
                        2 => (theme::ACCENT, theme::ACCENT),
                        3 => (theme::NEGATIVE, theme::NEGATIVE),
                        _ => (theme::SURFACE, theme::CHART_MUTED),
                    };
                    p.circle(egui::pos2(x + 6.0, y), 6.0, fill, egui::Stroke::new(2.0_f32, stroke));
                    p.text(egui::pos2(x, y + 14.0), egui::Align2::LEFT_TOP, *t, egui::FontId::new(14.0, theme::medium_family()), if *s == 3 { theme::NEGATIVE } else { theme::INK });
                    let d: String = if d.chars().count() > 34 { format!("{}…", d.chars().take(33).collect::<String>()) } else { d.clone() };
                    p.text(egui::pos2(x, y + 33.0), egui::Align2::LEFT_TOP, d, egui::FontId::new(13.0, egui::FontFamily::Proportional), theme::MUTED);
                }
            },
        );
        ui.add_space(12.0);
    }

    /// Testata: soggetto a sinistra, agente, data e numero a destra.
    fn header_top(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let role = if self.doc.kind.is_purchase() || self.doc.kind == DocKind::SelfInvoice { Some(PartyRole::Supplier) } else { Some(PartyRole::Customer) };
        let warehouses = cx.store.warehouses().unwrap_or_default();
        let transfer = self.doc.kind == DocKind::StockTransfer;
        let production = self.doc.kind == DocKind::Production;
        let draft = self.doc.status == DocStatus::Draft;
        let mut changed = false;
        fn cap(ui: &mut egui::Ui, t: &str) {
            ui.label(theme::bold(t).size(12.0));
        }
        let total = ui.available_width();
        let right_w = 640.0;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width((total - right_w).max(280.0));
                ui.spacing_mut().item_spacing.y = 2.0;
                if transfer {
                    ui.horizontal(|ui| {
                        cap(ui, "Da magazzino");
                        changed |= widgets::warehouse_combo(ui, "doc_wh_from", &mut self.doc.warehouse, &warehouses, true, 200.0);
                        cap(ui, "A magazzino");
                        changed |= widgets::warehouse_combo(ui, "doc_wh_to", &mut self.doc.to_warehouse, &warehouses, false, 200.0);
                    });
                } else if production {
                    ui.horizontal(|ui| {
                        cap(ui, "Magazzino");
                        changed |= widgets::warehouse_combo(ui, "doc_wh_prod", &mut self.doc.warehouse, &warehouses, true, 200.0);
                    });
                } else {
                    cap(ui, if role == Some(PartyRole::Supplier) { "Fornitore" } else { "Cliente" });
                    let current = self.doc.party.display_name();
                    if let Some(p) = widgets::party_picker(ui, cx.store, "doc_party", &current, role, (total - right_w - 20.0).clamp(260.0, 640.0)) {
                        self.set_party(cx.store, cx.msgs, p);
                    }
                    let p = &self.doc.party;
                    if !p.display_name().is_empty() {
                        let mut t = format!("{} · {}", p.full_address(), p.city_line());
                        if !p.vat_number.is_empty() {
                            t.push_str(&format!(" · P.IVA {}", p.vat_number));
                        } else if !p.tax_code.is_empty() {
                            t.push_str(&format!(" · CF {}", p.tax_code));
                        }
                        ui.label(RichText::new(widgets::truncate(&t, 110)).size(11.5).color(GRAY));
                    }
                }
            });
            ui.spacing_mut().item_spacing.x = 8.0;
            fn stack<R>(ui: &mut egui::Ui, w: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
                ui.allocate_ui_with_layout(egui::vec2(w, 46.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    add(ui)
                })
                .inner
            }
            if self.doc.kind.is_purchase() {
                stack(ui, 116.0, |ui| {
                    cap(ui, "N. doc. fornitore");
                    changed |= ui.add(egui::TextEdit::singleline(&mut self.doc.external_number).desired_width(110.0)).changed();
                });
            }
            if self.doc.kind.is_sale() && !transfer && !production && self.doc.kind != DocKind::RetailSale {
                let lists = cx.store.price_lists().unwrap_or_default();
                let base = cx.store.base_list_name();
                let shown = match self.doc.price_list_id {
                    Some(0) => base.clone(),
                    Some(id) => lists.iter().find(|l| l.id == id).map(|l| l.name.clone()).unwrap_or_else(|| base.clone()),
                    None => self.doc.party.price_list_id.and_then(|id| lists.iter().find(|l| l.id == id)).map(|l| l.name.clone()).unwrap_or_else(|| base.clone()),
                };
                let before = self.doc.price_list_id;
                stack(ui, 156.0, |ui| {
                    cap(ui, "Listino");
                    ui.add_enabled_ui(draft, |ui| {
                        egui::ComboBox::from_id_salt("doc_list").width(150.0).selected_text(shown).show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.doc.price_list_id, Some(0), &base);
                            for l in lists.iter().filter(|l| l.active) {
                                ui.selectable_value(&mut self.doc.price_list_id, Some(l.id), &l.name);
                            }
                        });
                    });
                });
                if before != self.doc.price_list_id {
                    changed = true;
                    let n = self.reprice_all(cx.store);
                    if n > 0 {
                        cx.msgs.ok(format!("Prezzi di {} righe aggiornati dal listino", n));
                    }
                }
            }
            if self.doc.kind.is_sale() && !transfer && !production {
                stack(ui, 156.0, |ui| {
                    cap(ui, "Agente");
                    let agents = cx.store.agents(false).unwrap_or_default();
                    let name = self.doc.agent_id.and_then(|id| agents.iter().find(|a| a.id == id)).map(|a| a.name.clone()).unwrap_or_else(|| "nessuno".into());
                    egui::ComboBox::from_id_salt("doc_agent").width(150.0).selected_text(name).show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut self.doc.agent_id, None, "nessuno").changed();
                        for a in &agents {
                            changed |= ui.selectable_value(&mut self.doc.agent_id, Some(a.id), format!("{} ({}%)", a.name, fmt_it_trim(a.rate, 0, 2))).changed();
                        }
                    });
                });
            }
            stack(ui, 124.0, |ui| {
                cap(ui, "Data");
                ui.horizontal(|ui| {
                    changed |= widgets::date_field(ui, "doc_date", &mut self.doc.date);
                });
            });
            stack(ui, 66.0, |ui| {
                cap(ui, "Numero");
                ui.add_enabled_ui(draft, |ui| {
                    changed |= ui.add(egui::TextEdit::singleline(&mut self.manual_number).hint_text("auto").desired_width(60.0)).changed();
                });
            });
            stack(ui, 60.0, |ui| {
                cap(ui, "Numeraz.");
                changed |= ui.add(egui::TextEdit::singleline(&mut self.doc.series).desired_width(50.0)).changed();
            });
        });
        if changed {
            self.dirty = true;
        }
    }

    /// Scheda Opzioni: tipo, prezzi, sconto, valuta, magazzino, provvigione, dichiarazione d'intento.
    fn tab_options(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let warehouses = cx.store.warehouses().unwrap_or_default();
        let draft = self.doc.status == DocStatus::Draft;
        let mut changed = false;
        egui::Grid::new("doc_opts").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("Tipo documento");
            ui.add_enabled_ui(draft, |ui| {
                egui::ComboBox::from_id_salt("doc_kind").width(240.0).selected_text(self.doc.kind.label()).show_ui(ui, |ui| {
                    for k in DocKind::ALL {
                        let special = |x: DocKind| matches!(x, DocKind::RetailSale | DocKind::StockTransfer | DocKind::Production);
                        if special(k) || special(self.doc.kind) {
                            continue;
                        }
                        if k.is_purchase() == self.doc.kind.is_purchase() && ui.selectable_value(&mut self.doc.kind, k, k.label()).changed() {
                            changed = true;
                            if k.has_transport() && self.doc.transport.is_none() {
                                self.doc.transport = Some(Transport::default());
                            }
                        }
                    }
                });
            });
            ui.end_row();
            ui.label("Prezzi");
            let mut inc = self.doc.prices_include_vat;
            if ui.checkbox(&mut inc, "IVA inclusa").changed() {
                self.set_price_mode(inc);
            }
            ui.end_row();
            ui.label("Sconto documento %");
            changed |= ui.add(egui::TextEdit::singleline(&mut self.doc.discount).desired_width(90.0).hint_text("es. 5 o 3+2")).changed();
            ui.end_row();
            if self.doc.kind.stock_effect() != 0 && warehouses.len() > 1 {
                ui.label("Magazzino");
                changed |= widgets::warehouse_combo(ui, "doc_wh", &mut self.doc.warehouse, &warehouses, true, 220.0);
                ui.end_row();
            }
            ui.label("Valuta");
            ui.horizontal(|ui| {
                let cur = self.doc.currency_code().to_string();
                egui::ComboBox::from_id_salt("doc_cur").width(70.0).selected_text(cur.clone()).show_ui(ui, |ui| {
                    for c in ["EUR", "USD", "GBP", "CHF", "JPY", "CNY", "CAD", "AUD", "SEK", "NOK", "DKK", "PLN", "CZK", "HUF", "RON", "TRY", "AED", "INR", "BRL"] {
                        if ui.selectable_label(cur == c, c).clicked() {
                            self.doc.currency = if c == "EUR" { String::new() } else { c.to_string() };
                            if c == "EUR" {
                                self.doc.exchange_rate = Decimal::ONE;
                            }
                            changed = true;
                        }
                    }
                });
                if self.doc.is_foreign_currency() {
                    ui.label("1 EUR =");
                    changed |= widgets::dec_field(ui, "doc_rate", &mut self.doc.exchange_rate, 70.0, 6);
                }
            });
            ui.end_row();
            if self.doc.kind.is_sale() && self.doc.agent_id.is_some() {
                ui.label("Provvigione %");
                ui.horizontal(|ui| {
                    let mut has = self.doc.commission_rate.is_some();
                    if ui.checkbox(&mut has, "specifica del documento").changed() {
                        self.doc.commission_rate = if has { Some(Decimal::ZERO) } else { None };
                        changed = true;
                    }
                    if let Some(r) = &mut self.doc.commission_rate {
                        changed |= widgets::dec_field(ui, "doc_comm", r, 60.0, 2);
                    }
                });
                ui.end_row();
            }
            if self.doc.kind.is_fiscal() && !self.doc.party.intents.is_empty() {
                ui.label("Dich. d'intento");
                let txt = if self.doc.intent_protocol.is_empty() { "nessuna".to_string() } else { self.doc.intent_protocol.clone() };
                ui.vertical(|ui| {
                    egui::ComboBox::from_id_salt("doc_intent").width(260.0).selected_text(txt).show_ui(ui, |ui| {
                        if ui.selectable_label(self.doc.intent_protocol.is_empty(), "nessuna").clicked() {
                            self.doc.intent_protocol.clear();
                            changed = true;
                        }
                        for i in &self.doc.party.intents {
                            if ui.selectable_label(self.doc.intent_protocol == i.protocol, format!("{} ({})", i.protocol, i.year)).clicked() {
                                self.doc.intent_protocol = i.protocol.clone();
                                changed = true;
                            }
                        }
                    });
                    if !self.doc.intent_protocol.is_empty() {
                        if let Some(pid) = self.doc.party_id {
                            let gen = cx.store.generation();
                            let fresh = matches!(&self.intent_cache, Some((g, p, _)) if *g == gen && *p == self.doc.intent_protocol);
                            if !fresh {
                                self.intent_cache = Some((gen, self.doc.intent_protocol.clone(), cx.store.intent_usage(pid, &self.doc.intent_protocol, Some(self.doc.id)).ok()));
                            }
                            if let Some(u) = self.intent_cache.as_ref().and_then(|c| c.2.clone()) {
                                let this = openfatt_store::intent_amount(&self.doc);
                                if let Some(res) = u.residual() {
                                    let left = res - this;
                                    let t = format!("plafond residuo € {} (questo documento € {})", eur(left), eur(this));
                                    ui.label(RichText::new(t).small().color(if left < Decimal::ZERO { RED } else { GRAY }));
                                }
                            }
                        }
                    }
                });
                ui.end_row();
            }
        });
        if changed {
            self.dirty = true;
        }
    }

    /// Scheda Proprietà fattura elettronica.
    fn tab_fe(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        self.sdi_track(ui, cx);
        let mut changed = false;
        egui::Grid::new("doc_fe").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("Tipo documento elettr.");
            let default = self.doc.kind.default_fe_type().unwrap_or("");
            let mut fe = if self.doc.fe_type.is_empty() { default.to_string() } else { self.doc.fe_type.clone() };
            if widgets::code_combo(ui, "fe_type", &mut fe, codes::TIPO_DOCUMENTO, 420.0, false) {
                self.doc.fe_type = if fe == default { String::new() } else { fe };
                changed = true;
            }
            ui.end_row();
            ui.label("Esigibilità IVA");
            if widgets::code_combo(ui, "esig", &mut self.doc.vat_liability, codes::ESIGIBILITA_IVA, 240.0, false) {
                changed = true;
            }
            ui.end_row();
            ui.label("Causale");
            changed |= ui.add(egui::TextEdit::multiline(&mut self.doc.causale).desired_rows(2).desired_width(620.0)).changed();
            ui.end_row();
        });
        if changed {
            self.dirty = true;
        }
        ui.add_space(6.0);
        self.refs_section(ui);
    }

    /// Stato dell'ordine e commento interno, sopra ai pulsanti.
    fn bottom_fields(&mut self, ui: &mut egui::Ui, totals: &Result<calc::Totals, calc::CalcError>) {
        let mut changed = false;
        let width = ui.available_width();
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width((width - 490.0).max(300.0));
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    if self.doc.kind.has_order_state() {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(RichText::new("Stato").size(11.5));
                            let eff = self.doc.order_status.effective(self.fulfillment.1 || self.fulfillment.0, self.fulfillment.0);
                            egui::ComboBox::from_id_salt("order_status").width(120.0).selected_text(eff.label()).show_ui(ui, |ui| {
                                for s in OrderStatus::ALL {
                                    changed |= ui.selectable_value(&mut self.doc.order_status, s, s.label()).changed();
                                }
                            });
                        });
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(RichText::new("Data prevista concl.").size(11.5));
                            ui.horizontal(|ui| {
                                let before = self.doc.expected_date;
                                widgets::opt_date_field(ui, "order_expected", &mut self.doc.expected_date);
                                changed |= before != self.doc.expected_date;
                            });
                        });
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new("Commento ad uso interno").size(11.5));
                        changed |= ui.add(egui::TextEdit::singleline(&mut self.doc.internal_notes).desired_width(f32::INFINITY)).changed();
                    });
                });
            });
            let show_margin = self.doc.kind.is_sale() && !self.cols.hide_margin;
            let margin = totals.as_ref().ok().map(|t| self.margin_totals(t));
            ui.allocate_ui_with_layout(egui::vec2(180.0, 74.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(180.0);
                ui.spacing_mut().item_spacing.y = 1.0;
                if let Some((rev, cost, missing)) = margin.filter(|_| show_margin) {
                    let m = rev - cost;
                    let pct = if rev.is_zero() { Decimal::ZERO } else { (m / rev * Decimal::ONE_HUNDRED).round_dp(1) };
                    let color = if m < Decimal::ZERO { RED } else { theme::INK };
                    if !cost.is_zero() || !rev.is_zero() {
                        ui.label(RichText::new(format!("Acquisto € {}", eur(cost))).size(12.0));
                        ui.label(RichText::new(format!("Margine € {}  ({}%)", eur(m), fmt_it_trim(pct, 0, 1))).size(12.5).strong().color(color));
                    }
                    if missing > 0 {
                        ui.label(RichText::new(format!("{} righe senza prezzo acquisto", missing)).size(11.0).color(ORANGE));
                    }
                }
            });
            ui.allocate_ui_with_layout(egui::vec2(290.0, 74.0), egui::Layout::top_down(egui::Align::Max), |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                fn row(ui: &mut egui::Ui, l: RichText, v: RichText) {
                    ui.allocate_ui_with_layout(egui::vec2(280.0, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_width(280.0);
                        ui.label(l);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(v));
                    });
                }
                match totals {
                    Err(e) => {
                        ui.label(RichText::new(e.to_string()).color(RED));
                    }
                    Ok(t) => {
                        let vat: Decimal = t.vat_rows.iter().map(|r| r.tax).sum();
                        row(ui, RichText::new("Tot. netto").size(12.0), RichText::new(format!("€ {}", eur(t.taxable))));
                        row(ui, RichText::new("Iva").size(12.0), RichText::new(format!("€ {}", eur(vat))));
                        let cur = if self.doc.is_foreign_currency() { self.doc.currency_code().to_string() } else { "€".to_string() };
                        row(ui, theme::bold("Totale documento").size(13.0), theme::bold(format!("{} {}", cur, eur(t.total))).size(14.0));
                        if t.net_to_pay != t.total {
                            row(ui, RichText::new("Netto a pagare").size(12.0), RichText::new(format!("€ {}", eur(t.net_to_pay))));
                        }
                    }
                }
            });
        });
        if changed {
            self.dirty = true;
        }
    }

    fn lines(&mut self, ui: &mut egui::Ui, cx: &mut Ctx, totals: &Option<calc::Totals>, height: f32) {
        let mut remove: Option<usize> = None;
        let mut swap: Option<(usize, usize)> = None;
        let mut changed = false;
        let n = self.doc.lines.len();
        let vats = self.vats.clone();
        let mut requote: Vec<usize> = Vec::new();
        let products: Vec<Option<Product>> = self.doc.lines.iter().map(|l| l.product_id.and_then(|id| cx.store.product(id).ok().flatten())).collect();
        // documenti senza effetto sul magazzino: si mostra la scelta come per un'uscita
        let effect = if self.doc.kind.stock_effect() == 0 { -1 } else { self.doc.kind.stock_effect() };
        let out_wh = cx.store.resolve_warehouse(&self.doc.warehouse).ok();
        // colonne facoltative: compaiono solo se servono
        let show_track = products.iter().flatten().any(|p| p.tracking != Tracking::None || !p.variants.is_empty());
        let show_wh = !self.doc.withholdings.is_empty();
        let show_pf = self.doc.pension_fund.is_some();
        let sale = self.doc.kind.is_sale() && !self.doc.kind.is_internal();
        let show_cost = sale && !self.cols.hide_cost;
        let show_net = !self.doc.kind.is_internal() && !self.cols.hide_net;
        let show_margin = sale && !self.cols.hide_margin;
        let blanks = 5 + show_track as usize + show_wh as usize + show_pf as usize + show_cost as usize + show_net as usize;
        let desc_w = (ui.available_width() - 700.0 - if show_track { 170.0 } else { 0.0 } - if show_wh { 36.0 } else { 0.0 } - if show_pf { 36.0 } else { 0.0 } - if show_cost { 92.0 } else { 0.0 } - if show_net { 82.0 } else { 0.0 } - if show_margin { 116.0 } else { 0.0 }).max(180.0);
        let mut price_mode: Option<bool> = None;
        let include_vat = self.doc.prices_include_vat;
        egui::ScrollArea::both().id_salt("lines_scroll").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("lines").striped(true).num_columns(13).spacing([6.0, 3.0]).show(ui, |ui| {
                let mut heads = vec!["Cod.", "Descrizione"];
                if show_track {
                    heads.push("Variante / lotto / serie");
                }
                heads.extend(["Q.tà", "U.M."]);
                if show_cost {
                    heads.push("Prezzo acq.");
                }
                heads.extend(["#prezzo", "Sconti %"]);
                if show_net {
                    heads.push("Netto sc.");
                }
                heads.push("IVA");
                if show_wh {
                    heads.push("Rit.");
                }
                if show_pf {
                    heads.push("Cassa");
                }
                heads.push("Importo");
                if show_margin {
                    heads.push("Margine");
                }
                heads.push("");
                for h in heads {
                    if h == "#prezzo" {
                        // come nell'intestazione "Prezzo netto": si sceglie netto o ivato
                        ui.scope(|ui| {
                            ui.visuals_mut().button_frame = false;
                            ui.menu_button(RichText::new(format!("{} {}", if include_vat { "Prezzo ivato" } else { "Prezzo netto" }, icon::CARET_DOWN)).strong().color(ui.visuals().hyperlink_color), |ui| {
                                if ui.selectable_label(!include_vat, "Prezzo netto (IVA esclusa)").clicked() {
                                    price_mode = Some(false);
                                    ui.close_menu();
                                }
                                if ui.selectable_label(include_vat, "Prezzo ivato (IVA inclusa)").clicked() {
                                    price_mode = Some(true);
                                    ui.close_menu();
                                }
                            });
                        });
                    } else {
                        ui.label(RichText::new(h).strong());
                    }
                }
                ui.end_row();
                for i in 0..n {
                    let total = totals.as_ref().and_then(|t| t.lines.iter().find(|l| l.index == Some(i))).map(|l| l.total);
                    let l = &mut self.doc.lines[i];
                    if l.kind == LineKind::Text || l.kind == LineKind::Subtotal {
                        let sub = l.kind == LineKind::Subtotal;
                        ui.label(RichText::new(if sub { "subtotale" } else { "testo" }).small().color(GRAY));
                        if ui.add_sized([desc_w, 20.0], egui::TextEdit::singleline(&mut l.description).hint_text(if sub { "Subtotale" } else { "" })).changed() {
                            changed = true;
                        }
                        for _ in 0..blanks {
                            ui.label("");
                        }
                        let st = totals.as_ref().and_then(|t| t.lines.iter().find(|x| x.index == Some(i))).and_then(|x| x.subtotal);
                        ui.label(theme::medium(st.map(eur).unwrap_or_default()));
                        if show_margin {
                            ui.label("");
                        }
                    } else if let Some(pct) = l.calc_percent {
                        ui.label(RichText::new("calcolata").small().color(GRAY));
                        if ui.add_sized([desc_w, 20.0], egui::TextEdit::singleline(&mut l.description)).changed() {
                            changed = true;
                        }
                        if show_track {
                            ui.label(RichText::new(format!("{}% delle righe sopra", fmt_it_trim(pct, 0, 4))).small().color(GRAY));
                        }
                        let mut p = pct;
                        if widgets::dec_field(ui, ("pc", i), &mut p, 70.0, 4) {
                            l.calc_percent = Some(p);
                            changed = true;
                        }
                        ui.label("%");
                        if show_cost {
                            ui.label("");
                        }
                        ui.label("");
                        ui.label("");
                        if show_net {
                            ui.label("");
                        }
                        if widgets::vat_combo(ui, ("v", i), &mut l.vat, &vats, 70.0) {
                            changed = true;
                        }
                        if show_wh && ui.checkbox(&mut l.withholding, "").changed() {
                            changed = true;
                        }
                        if show_pf && ui.checkbox(&mut l.pension_fund, "").changed() {
                            changed = true;
                        }
                        ui.label(total.map(eur).unwrap_or_else(|| "-".into()));
                        if show_margin {
                            ui.label("");
                        }
                    } else {
                        if ui.add_sized([90.0, 20.0], egui::TextEdit::singleline(&mut l.code)).changed() {
                            changed = true;
                        }
                        if ui.add_sized([desc_w, 20.0], egui::TextEdit::singleline(&mut l.description)).changed() {
                            changed = true;
                        }
                        let incoming = effect * if l.qty.is_sign_negative() { -1 } else { 1 } > 0;
                        if show_track && super::tracking::cell(ui, cx.store, products[i].as_ref(), l, i, incoming, out_wh.as_deref()) {
                            changed = true;
                        }
                        if widgets::dec_field(ui, ("q", i), &mut l.qty, 70.0, 4) {
                            changed = true;
                            // scaglioni di quantità: si ricalcola se il prezzo viene da listino o condizioni
                            if !l.price_note.is_empty() && l.price_note != "Prezzo inserito a mano" {
                                requote.push(i);
                            }
                        }
                        if ui.add(egui::TextEdit::singleline(&mut l.unit).desired_width(36.0)).changed() {
                            changed = true;
                        }
                        if show_cost && widgets::dec_field(ui, ("c", i), &mut l.unit_cost, 80.0, 4) {
                            changed = true;
                        }
                        if widgets::dec_field(ui, ("p", i), &mut l.unit_price, 90.0, 6) {
                            changed = true;
                            if l.product_id.is_some() {
                                l.price_note = "Prezzo inserito a mano".into();
                            }
                        }
                        let disc_ok = calc::parse_discounts(&l.discounts).is_ok();
                        if ui.add(egui::TextEdit::singleline(&mut l.discounts).desired_width(70.0).text_color(if disc_ok { ui.visuals().text_color() } else { RED })).changed() {
                            changed = true;
                        }
                        if show_net {
                            let net = totals.as_ref().and_then(|t| t.lines.iter().find(|x| x.index == Some(i))).map(|x| x.unit_discounted);
                            ui.label(net.map(|n| fmt_it_trim(n.round_dp(4), 2, 4)).unwrap_or_else(|| "-".into()));
                        }
                        if widgets::vat_combo(ui, ("v", i), &mut l.vat, &vats, 70.0) {
                            changed = true;
                        }
                        if show_wh && ui.checkbox(&mut l.withholding, "").changed() {
                            changed = true;
                        }
                        if show_pf && ui.checkbox(&mut l.pension_fund, "").changed() {
                            changed = true;
                        }
                        let r = ui.label(total.map(eur).unwrap_or_else(|| "-".into()));
                        if !l.price_note.is_empty() {
                            r.on_hover_text(format!("Prezzo: {}", l.price_note));
                        }
                        if show_margin {
                            match total.filter(|_| !l.unit_cost.is_zero()) {
                                Some(t) => {
                                    let m = t - l.unit_cost * l.qty;
                                    let pct = if t.is_zero() { Decimal::ZERO } else { (m / t * Decimal::ONE_HUNDRED).round_dp(1) };
                                    ui.label(RichText::new(format!("{} ({}%)", eur(m), fmt_it_trim(pct, 0, 1))).color(if m < Decimal::ZERO { RED } else { ui.visuals().text_color() }));
                                }
                                None => {
                                    ui.label(RichText::new("-").color(GRAY));
                                }
                            }
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.menu_button(icon::DOTS_THREE, |ui| {
                            let item = self.doc.lines[i].kind == LineKind::Item && self.doc.lines[i].calc_percent.is_none();
                            if item && ui.button(format!("{}  Calcola il prezzo dal totale...", icon::CALCULATOR)).clicked() {
                                ui.close_menu();
                                self.tools.reverse_line = Some((i, String::new(), true));
                            }
                            if item && self.doc.lines[i].product_id.is_some() {
                                if ui.button(format!("{}  Ultimi prezzi praticati...", icon::CLOCK_COUNTER_CLOCKWISE)).clicked() {
                                    ui.close_menu();
                                    let lp = match (self.doc.party_id, self.doc.lines[i].product_id) {
                                        (Some(party), Some(pid)) => cx.store.last_prices(party, pid, self.doc.kind.is_purchase(), 10).unwrap_or_default(),
                                        _ => vec![],
                                    };
                                    self.tools.last_prices = Some((i, lp));
                                }
                                if ui.button(format!("{}  Quantità a confezioni...", icon::PACKAGE)).clicked() {
                                    ui.close_menu();
                                    let t = self.doc.lines[i].packs.map(|p| fmt_it_trim(p, 0, 3)).unwrap_or_default();
                                    self.tools.packs = Some((i, t));
                                }
                            }
                            if item && ui.button(format!("{}  Sconto merce (omaggio)", icon::GIFT)).on_hover_text("Aggiunge la stessa merce con sconto del 100%").clicked() {
                                ui.close_menu();
                                let mut c = openfatt_store::copy_line(&self.doc.lines[i]);
                                c.discounts = "100".into();
                                c.description = format!("{} - sconto merce", c.description.trim());
                                c.eco_fee = Decimal::ZERO;
                                self.doc.lines.insert(i + 1, c);
                                changed = true;
                            }
                            if ui.button(format!("{}  Duplica la riga", icon::COPY)).clicked() {
                                ui.close_menu();
                                let c = openfatt_store::copy_line(&self.doc.lines[i]);
                                self.doc.lines.insert(i + 1, c);
                                changed = true;
                            }
                            if ui.button(format!("{}  Inserisci un subtotale qui sotto", icon::LIST_PLUS)).clicked() {
                                ui.close_menu();
                                self.doc.lines.insert(i + 1, DocLine { kind: LineKind::Subtotal, ..Default::default() });
                                changed = true;
                            }
                            if i > 0 && ui.button(format!("{}  Sposta in cima", icon::ARROW_LINE_UP)).clicked() {
                                ui.close_menu();
                                let l = self.doc.lines.remove(i);
                                self.doc.lines.insert(0, l);
                                changed = true;
                            }
                            if i + 1 < n && ui.button(format!("{}  Sposta in fondo", icon::ARROW_LINE_DOWN)).clicked() {
                                ui.close_menu();
                                let l = self.doc.lines.remove(i);
                                self.doc.lines.push(l);
                                changed = true;
                            }
                        });
                        if i > 0 && ui.small_button("\u{E08E}").clicked() {
                            swap = Some((i - 1, i));
                        }
                        if i + 1 < n && ui.small_button("\u{E03E}").clicked() {
                            swap = Some((i, i + 1));
                        }
                        if ui.small_button("\u{E4A6}").on_hover_text("Elimina riga").clicked() {
                            remove = Some(i);
                        }
                    });
                    ui.end_row();
                }
            });
        });
        if let Some(m) = price_mode {
            self.set_price_mode(m);
        }
        for i in requote {
            if let Some(p) = products.get(i).cloned().flatten() {
                let (qty, rate) = (self.doc.lines[i].qty, self.doc.lines[i].vat.rate);
                let (price, disc, note) = self.priced(cx.store, &p, qty, rate);
                let l = &mut self.doc.lines[i];
                l.unit_price = price;
                l.discounts = disc;
                l.price_note = note;
            }
        }
        if let Some((a, b)) = swap {
            self.doc.lines.swap(a, b);
            changed = true;
        }
        if let Some(i) = remove {
            self.doc.lines.remove(i);
            changed = true;
        }

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.label(theme::muted("Aggiungi riga:"));
            if theme::ghost(ui, true, icon::PENCIL_LINE, "Manuale").clicked() {
                let vat = self.default_vat();
                self.doc.lines.push(DocLine { vat, discounts: self.doc.party.default_discount.clone(), pension_fund: self.doc.pension_fund.is_some(), withholding: !self.doc.withholdings.is_empty(), ..Default::default() });
                changed = true;
            }
            let search_id = ui.make_persistent_id("prod_search");
            let mut search: String = ui.data_mut(|d| d.get_temp(search_id)).unwrap_or_default();
            let mut picked: Option<Product> = None;
            egui::ComboBox::from_id_salt("add_product").width(150.0).height(400.0).selected_text(format!("{}  Prodotti", icon::PACKAGE)).show_ui(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut search).hint_text("codice o descrizione").desired_width(240.0)).request_focus();
                for p in cx.store.products_page(&search, false, 60).unwrap_or_default() {
                    if ui.selectable_label(false, format!("{}  {}  € {}", p.code, p.description, eur(p.price))).clicked() {
                        picked = Some(p);
                    }
                }
            });
            ui.data_mut(|d| d.insert_temp(search_id, search));
            if let Some(p) = picked {
                let lines = self.lines_for_product(cx.store, &p);
                self.doc.lines.extend(lines);
                changed = true;
            }
            let r = ui.add(egui::TextEdit::singleline(&mut self.quick_code).desired_width(120.0).hint_text("Cod. barre + Invio"));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.quick_code.trim().is_empty() {
                match cx.store.find_by_code(&self.quick_code) {
                    Ok(Some((p, variant))) => {
                        // stesso articolo e variante come ultima riga: si incrementa la quantità
                        if let Some(last) = self.doc.lines.last_mut().filter(|l| l.product_id == Some(p.id) && l.variant == variant && p.tracking != Tracking::Serial) {
                            last.qty += Decimal::ONE;
                        } else {
                            let mut lines = self.lines_for_product(cx.store, &p);
                            if let Some(l) = lines.first_mut() {
                                l.variant = variant;
                            }
                            self.doc.lines.extend(lines);
                        }
                        changed = true;
                    }
                    _ => cx.msgs.warn(format!("Nessun articolo con codice '{}'", self.quick_code.trim())),
                }
                self.quick_code.clear();
                r.request_focus();
            }
            if !self.doc.kind.is_internal() && theme::ghost(ui, true, icon::PERCENT, "Calcolata").on_hover_text("Percentuale delle righe precedenti: trasporto, imballo...").clicked() {
                self.tools.percent = Some(("Spese di trasporto".into(), String::new()));
            }
            if theme::ghost(ui, true, icon::NOTE, "Nota").clicked() {
                self.doc.lines.push(DocLine { kind: LineKind::Text, ..Default::default() });
                changed = true;
            }
            egui::menu::menu_custom_button(ui, egui::Button::new(format!("{}  Utilità", icon::WRENCH)).frame(false).min_size(egui::vec2(0.0, 27.0)), |ui| {
                if !self.doc.kind.is_internal() {
                    if ui.button(format!("{}  Subtotale", icon::LIST_PLUS)).clicked() {
                        ui.close_menu();
                        self.doc.lines.push(DocLine { kind: LineKind::Subtotal, ..Default::default() });
                        changed = true;
                    }
                    if ui.button(format!("{}  Porta il totale a...", icon::CALCULATOR)).clicked() {
                        ui.close_menu();
                        self.tools.target_total = Some(String::new());
                    }
                }
                if self.doc.lines.iter().any(|l| l.product_id.is_some()) && ui.button(format!("{}  Prezzi da listino", icon::TAG)).clicked() {
                    ui.close_menu();
                    let n = self.reprice_all(cx.store);
                    cx.msgs.ok(format!("Prezzi ricalcolati su {} righe", n));
                    changed = true;
                }
                if ui.button(format!("{}  Copia righe da...", icon::COPY)).clicked() {
                    ui.close_menu();
                    self.tools.copy = Some(Default::default());
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::menu::menu_custom_button(ui, egui::Button::new(RichText::new("Colonne...").color(ui.visuals().hyperlink_color)).frame(false), |ui| {
                    let before = self.cols;
                    let mut c = !self.cols.hide_cost;
                    let mut n = !self.cols.hide_net;
                    let mut m = !self.cols.hide_margin;
                    ui.checkbox(&mut c, "Prezzo di acquisto");
                    ui.checkbox(&mut n, "Prezzo netto scontato");
                    ui.checkbox(&mut m, "Margine");
                    self.cols = LineCols { hide_cost: !c, hide_net: !n, hide_margin: !m };
                    if before != self.cols {
                        let _ = cx.store.set_setting("doc_line_columns", &self.cols);
                    }
                });
            });
        });
        if changed {
            self.dirty = true;
            if self.doc.lines.iter().any(|l| l.vat.split) {
                self.doc.vat_liability = "S".into();
            }
        }
    }

    fn fiscal_section(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        egui::CollapsingHeader::new("Ritenute, cassa previdenziale e bollo").default_open(self.doc.pension_fund.is_some() || !self.doc.withholdings.is_empty()).show(ui, |ui| {
            // cassa
            let mut has_pf = self.doc.pension_fund.is_some();
            if ui.checkbox(&mut has_pf, "Contributo cassa previdenziale / rivalsa INPS").changed() {
                self.doc.pension_fund = if has_pf { Some(PensionFund { vat: self.default_vat(), ..Default::default() }) } else { None };
                for l in self.doc.lines.iter_mut() {
                    l.pension_fund = has_pf;
                }
                changed = true;
            }
            let vats = self.vats.clone();
            if let Some(pf) = &mut self.doc.pension_fund {
                ui.horizontal(|ui| {
                    ui.label("Cassa");
                    changed |= widgets::code_combo(ui, "pf_kind", &mut pf.kind, codes::TIPO_CASSA, 280.0, false);
                    ui.label("Aliquota %");
                    changed |= widgets::dec_field(ui, "pf_rate", &mut pf.rate, 60.0, 2);
                    ui.label("IVA sul contributo");
                    changed |= widgets::vat_combo(ui, "pf_vat", &mut pf.vat, &vats, 90.0);
                    changed |= ui.checkbox(&mut pf.subject_to_withholding, "soggetto a ritenuta").changed();
                });
            }
            ui.separator();
            // ritenute
            let mut remove = None;
            for (i, w) in self.doc.withholdings.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label("Ritenuta");
                    changed |= widgets::code_combo(ui, &format!("w_kind{}", i), &mut w.kind, codes::TIPO_RITENUTA, 220.0, false);
                    ui.label("Aliquota %");
                    changed |= widgets::dec_field(ui, ("w_rate", i), &mut w.rate, 60.0, 2);
                    ui.label("sul % dell'imponibile");
                    changed |= widgets::dec_field(ui, ("w_base", i), &mut w.base_percent, 60.0, 2);
                    ui.label("Causale");
                    changed |= widgets::code_combo(ui, &format!("w_caus{}", i), &mut w.payment_reason, codes::CAUSALE_PAGAMENTO, 260.0, false);
                    if ui.small_button("\u{E4A6}").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                self.doc.withholdings.remove(i);
                changed = true;
            }
            if self.doc.withholdings.len() < 2 && ui.button("\u{E3D4} Ritenuta d'acconto").clicked() {
                self.doc.withholdings.push(Withholding::default());
                for l in self.doc.lines.iter_mut().filter(|l| l.kind == LineKind::Item) {
                    l.withholding = true;
                }
                changed = true;
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Imposta di bollo");
                egui::ComboBox::from_id_salt("stamp_mode")
                    .selected_text(match self.doc.stamp.mode {
                        StampMode::Auto => "Automatica (oltre 77,47 € esenti)",
                        StampMode::Yes => "Sempre",
                        StampMode::No => "Mai",
                    })
                    .show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut self.doc.stamp.mode, StampMode::Auto, "Automatica (oltre 77,47 € esenti)").changed();
                        changed |= ui.selectable_value(&mut self.doc.stamp.mode, StampMode::Yes, "Sempre").changed();
                        changed |= ui.selectable_value(&mut self.doc.stamp.mode, StampMode::No, "Mai").changed();
                    });
                ui.label("Importo");
                changed |= widgets::dec_field(ui, "stamp_amount", &mut self.doc.stamp.amount, 60.0, 2);
                changed |= ui.checkbox(&mut self.doc.stamp.charge_to_customer, "addebita al cliente").changed();
            });
        });
        if changed {
            self.dirty = true;
        }
    }

    fn transport_section(&mut self, ui: &mut egui::Ui) {
        if !self.doc.kind.has_transport() {
            return;
        }
        let mut changed = false;
        egui::CollapsingHeader::new("Trasporto").default_open(true).show(ui, |ui| {
            let t = self.doc.transport.get_or_insert_with(Transport::default);
            egui::Grid::new("transport").num_columns(6).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Causale");
                egui::ComboBox::from_id_salt("tr_reason").width(180.0).selected_text(if t.reason.is_empty() { "-" } else { t.reason.as_str() }).show_ui(ui, |ui| {
                    for r in ["Vendita", "Reso", "Conto visione", "Conto lavorazione", "Riparazione", "Omaggio", "Trasferimento"] {
                        changed |= ui.selectable_value(&mut t.reason, r.to_string(), r).changed();
                    }
                });
                ui.label("A cura del");
                egui::ComboBox::from_id_salt("tr_by").width(140.0).selected_text(if t.by.is_empty() { "-" } else { t.by.as_str() }).show_ui(ui, |ui| {
                    for r in ["Mittente", "Destinatario", "Vettore"] {
                        changed |= ui.selectable_value(&mut t.by, r.to_string(), r).changed();
                    }
                });
                ui.label("Vettore");
                changed |= ui.add(egui::TextEdit::singleline(&mut t.carrier).desired_width(200.0)).changed();
                ui.end_row();
                ui.label("Colli");
                let mut colli = t.packages.map(Decimal::from).unwrap_or_default();
                if widgets::dec_field(ui, "colli", &mut colli, 60.0, 0) {
                    t.packages = rust_decimal::prelude::ToPrimitive::to_u32(&colli).filter(|n| *n > 0);
                    changed = true;
                }
                ui.label("Aspetto beni");
                changed |= ui.add(egui::TextEdit::singleline(&mut t.appearance).desired_width(140.0)).changed();
                ui.label("Porto");
                changed |= ui.add(egui::TextEdit::singleline(&mut t.port).desired_width(120.0).hint_text("franco / assegnato")).changed();
                ui.end_row();
                ui.label("Peso lordo kg");
                let mut w = t.gross_weight.unwrap_or_default();
                if widgets::dec_field(ui, "peso", &mut w, 70.0, 3) {
                    t.gross_weight = if w.is_zero() { None } else { Some(w) };
                    changed = true;
                }
                ui.label("Inizio trasporto");
                let mut d = t.start.map(|x| x.date());
                widgets::opt_date_field(ui, "tr_start", &mut d);
                let nd = d.map(|x| x.and_hms_opt(8, 0, 0).unwrap());
                if nd.map(|x| x.date()) != t.start.map(|x| x.date()) {
                    t.start = nd;
                    changed = true;
                }
                ui.label("");
                ui.label("");
                ui.end_row();
            });
        });
        if changed {
            self.dirty = true;
        }
    }

    fn refs_section(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        egui::CollapsingHeader::new("Riferimenti (ordine, contratto, CIG/CUP, fattura collegata)").default_open(!self.doc.related.is_empty()).show(ui, |ui| {
            let mut remove = None;
            for (i, r) in self.doc.related.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt(("rel_kind", i)).width(120.0).selected_text(r.kind.clone()).show_ui(ui, |ui| {
                        for k in ["ORDINE", "CONTRATTO", "CONVENZIONE", "RICEZIONE", "FATTURA"] {
                            changed |= ui.selectable_value(&mut r.kind, k.to_string(), k).changed();
                        }
                    });
                    ui.label("N.");
                    changed |= ui.add(egui::TextEdit::singleline(&mut r.number).desired_width(110.0)).changed();
                    ui.label("Data");
                    let before = r.date;
                    widgets::opt_date_field(ui, &format!("rel_date{}", i), &mut r.date);
                    changed |= before != r.date;
                    ui.label("CIG");
                    changed |= ui.add(egui::TextEdit::singleline(&mut r.cig).desired_width(110.0)).changed();
                    ui.label("CUP");
                    changed |= ui.add(egui::TextEdit::singleline(&mut r.cup).desired_width(130.0)).changed();
                    if ui.small_button("\u{E4A6}").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                self.doc.related.remove(i);
                changed = true;
            }
            if ui.button("\u{E3D4} Riferimento").clicked() {
                self.doc.related.push(RelatedDoc { kind: if self.doc.kind.is_credit() { "FATTURA".into() } else { "ORDINE".into() }, ..Default::default() });
                changed = true;
            }
        });
        if changed {
            self.dirty = true;
        }
    }

    fn totals_panel(&self, ui: &mut egui::Ui, totals: &Result<calc::Totals, calc::CalcError>) {
        theme::card_frame().show(ui, |ui| ui.vertical(|ui| {
            ui.set_min_width(320.0);
            ui.label(theme::medium("Totali").size(16.0));
            ui.add_space(6.0);
            match totals {
                Err(e) => {
                    ui.label(RichText::new(format!("\u{E4E0} {}", e)).color(RED));
                }
                Ok(t) => {
                    egui::Grid::new("tot").num_columns(2).spacing([20.0, 4.0]).show(ui, |ui| {
                        for r in &t.vat_rows {
                            let label = match &r.natura {
                                Some(n) => format!("Imponibile {}", n),
                                None => format!("Imponibile {}%", fmt_it_trim(r.rate, 0, 2)),
                            };
                            ui.label(label);
                            ui.label(eur(r.taxable));
                            ui.end_row();
                            if !r.rate.is_zero() {
                                ui.label(format!("IVA {}%{}", fmt_it_trim(r.rate, 0, 2), if r.liability == "S" { " (scissione)" } else { "" }));
                                ui.label(eur(r.tax));
                                ui.end_row();
                            }
                        }
                        if let Some(pf) = &t.pension_fund {
                            ui.label(RichText::new(format!("di cui contributo {} {}%", pf.kind, fmt_it_trim(pf.rate, 0, 2))).color(GRAY));
                            ui.label(RichText::new(eur(pf.amount)).color(GRAY));
                            ui.end_row();
                        }
                        if let Some(s) = t.stamp {
                            ui.label(RichText::new(if t.stamp_charged { "Bollo (addebitato)" } else { "Bollo virtuale" }).color(GRAY));
                            ui.label(RichText::new(eur(s)).color(GRAY));
                            ui.end_row();
                        }
                        ui.label(theme::medium("Totale documento"));
                        ui.label(theme::bold(format!("{} {}", if self.doc.is_foreign_currency() { self.doc.currency_code() } else { "€" }, eur(t.total))).size(24.0));
                        ui.end_row();
                        for w in &t.withholdings {
                            ui.label(format!("Ritenuta {} {}% su {}", w.kind, fmt_it_trim(w.rate, 0, 2), eur(w.base)));
                            ui.label(format!("-{}", eur(w.amount)));
                            ui.end_row();
                        }
                        if self.doc.is_foreign_currency() {
                            ui.label(RichText::new("Controvalore in euro").color(GRAY));
                            ui.label(RichText::new(format!("€ {}", eur(self.doc.to_eur(t.total)))).color(GRAY));
                            ui.end_row();
                        }
                        if t.net_to_pay != t.total {
                            ui.label(RichText::new("Netto a pagare").strong());
                            ui.label(RichText::new(format!("€ {}", eur(t.net_to_pay))).strong());
                            ui.end_row();
                        }
                    });
                    for w in &t.warnings {
                        ui.label(RichText::new(format!("\u{E4E0} {}", w)).color(ORANGE).small());
                    }
                    if self.doc.kind.is_sale() {
                        let (rev, cost, _) = self.margin_totals(t);
                        if !cost.is_zero() {
                            ui.label(RichText::new(format!("Margine € {}", eur(rev - cost))).small().color(GRAY));
                        }
                    }
                }
            }
            if !self.doc.due_dates.is_empty() {
                ui.add_space(6.0);
                theme::rule(ui);
                ui.add_space(6.0);
                ui.label(theme::medium("Scadenze"));
                for d in &self.doc.due_dates {
                    ui.label(format!(
                        "{}  € {}  {}{}",
                        d.date.format("%d/%m/%Y"),
                        eur(d.amount),
                        codes::describe(codes::MODALITA_PAGAMENTO, &d.mp_code),
                        if d.is_paid() { "  \u{E182} pagata" } else if !d.paid.is_zero() { "  (parziale)" } else { "" }
                    ));
                }
            } else if let (Some(pid), Ok(t)) = (self.doc.payment_id, totals) {
                if self.doc.kind.has_due_dates() {
                    if let Some(p) = self.payments.iter().find(|p| p.id == pid) {
                        ui.separator();
                        ui.label(RichText::new("Scadenze previste").strong());
                        for d in calc::due_dates(self.doc.date, t.net_to_pay, p) {
                            ui.label(RichText::new(format!("{}  € {}", d.date.format("%d/%m/%Y"), eur(d.amount))).color(GRAY));
                        }
                    }
                }
            }
        }));
    }

    /// Quantità e valore dei documenti interni (produzione e trasferimenti).
    fn internal_info(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
            if self.doc.kind == DocKind::Production {
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(theme::medium("Componenti da scaricare").size(16.0));
                    let mut need: Vec<(String, String, Decimal, Decimal)> = Vec::new();
                    let mut value = Decimal::ZERO;
                    for l in self.doc.lines.iter().filter(|l| l.kind == LineKind::Item) {
                        let Some(p) = l.product_id.and_then(|id| cx.store.product(id).ok().flatten()) else { continue };
                        value += cx.store.bom_cost(&p).unwrap_or_default() * l.qty;
                        for c in &p.components {
                            if let Some(cp) = cx.store.product(c.product_id).ok().flatten() {
                                let q = c.qty * l.qty;
                                if let Some(x) = need.iter_mut().find(|x| x.0 == cp.code) {
                                    x.2 += q;
                                } else {
                                    need.push((cp.code.clone(), cp.description.clone(), q, cx.store.stock_of(cp.id).unwrap_or_default()));
                                }
                            }
                        }
                    }
                    if need.is_empty() {
                        ui.label(RichText::new("Aggiungere i prodotti con distinta base da produrre.").color(GRAY));
                    }
                    egui::Grid::new("prod_need").striped(true).num_columns(4).spacing([16.0, 4.0]).show(ui, |ui| {
                        for h in ["Codice", "Componente", "Da scaricare", "Giacenza"] {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();
                        for (code, desc, q, stock) in &need {
                            ui.label(code);
                            ui.label(desc);
                            ui.label(fmt_it_trim(*q, 0, 3));
                            ui.label(RichText::new(fmt_it_trim(*stock, 0, 3)).color(if stock < q { RED } else { GRAY }));
                            ui.end_row();
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(format!("Valore della produzione al costo dei componenti: € {}", eur(value)));
                });
            }
            if self.doc.kind == DocKind::StockTransfer {
                // nessun valore fiscale: quantità e valore della merce al costo
                let qty: Decimal = self.doc.lines.iter().filter(|l| l.kind == LineKind::Item).map(|l| l.qty).sum();
                let value: Decimal = self
                    .doc
                    .lines
                    .iter()
                    .filter(|l| l.kind == LineKind::Item)
                    .map(|l| l.qty * l.product_id.and_then(|id| cx.store.product(id).ok().flatten()).map(|p| p.cost).unwrap_or(l.unit_cost))
                    .sum();
                egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(format!("Quantità trasferita: {}", fmt_it_trim(qty, 0, 3))).strong());
                        ui.label(format!("Valore della merce al costo: € {}", eur(value)));
                        ui.label(RichText::new("Il trasferimento non ha effetti fiscali: sposta la merce tra due magazzini dell'azienda in un'unica operazione.").small().color(GRAY));
                    });
                });
            }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        if self.confirm_close {
            match widgets::confirm(ui.ctx(), "Modifiche non salvate", "Chiudere il documento senza salvare le modifiche?") {
                Some(true) => {
                    self.confirm_close = false;
                    cx.nav.push(Nav::CloseEditor);
                }
                Some(false) => self.confirm_close = false,
                None => {}
            }
        }
        self.title_bar(ui, cx);
        if let super::linetools::LineAction::Changed = self.tools.show(ui.ctx(), cx.store, cx.msgs, &mut self.doc) {
            self.dirty = true;
        }
        let totals = calc::compute(&self.doc);
        let locked = self.lock.is_some() || self.doc.status == DocStatus::Cancelled;
        let internal = self.doc.kind.is_internal();
        egui::TopBottomPanel::bottom("doc_actions").show_separator_line(true).frame(egui::Frame::new().inner_margin(egui::Margin { left: 0, right: 0, top: 4, bottom: 0 })).show_inside(ui, |ui| self.action_bar(ui, cx));
        if !internal {
            egui::TopBottomPanel::bottom("doc_bottom").show_separator_line(true).exact_height(84.0).frame(egui::Frame::new().inner_margin(egui::Margin { left: 0, right: 0, top: 4, bottom: 4 })).show_inside(ui, |ui| {
                ui.add_enabled_ui(!locked, |ui| self.bottom_fields(ui, &totals));
            });
        }
        egui::TopBottomPanel::top("doc_head").show_separator_line(false).frame(egui::Frame::new().inner_margin(egui::Margin { left: 0, right: 0, top: 0, bottom: 4 })).show_inside(ui, |ui| {
            ui.add_enabled_ui(!locked, |ui| self.header_top(ui, cx));
            if !internal {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let mut tabs = vec![(DocTab::Lines, "Righe documento"), (DocTab::Payment, "Pagamento")];
                    if self.doc.kind.has_transport() {
                        tabs.push((DocTab::Transport, "Trasporto"));
                    }
                    tabs.push((DocTab::Notes, "Note"));
                    tabs.push((DocTab::Addresses, "Indirizzi"));
                    if self.doc.kind.is_fiscal() || self.doc.kind.is_purchase() {
                        tabs.push((DocTab::FeProps, "Proprietà fattura elettr."));
                    }
                    tabs.push((DocTab::Options, "Opzioni"));
                    for (t, label) in tabs {
                        ui.selectable_value(&mut self.tab, t, label);
                    }
                });
            }
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::SURFACE).inner_margin(6)).show_inside(ui, |ui| {
            if internal {
                self.tab = DocTab::Lines;
            }
            match self.tab {
                DocTab::Lines => {
                    let h = (ui.available_height() - 40.0 - if internal { 170.0 } else { 0.0 }).max(120.0);
                    ui.add_enabled_ui(!locked, |ui| {
                        let t = totals.as_ref().ok().cloned();
                        self.lines(ui, cx, &t, h);
                    });
                    if internal {
                        self.internal_info(ui, cx);
                    }
                }
                other => {
                    egui::ScrollArea::vertical().id_salt("doc_tab_scroll").auto_shrink([false, false]).show(ui, |ui| match other {
                        DocTab::Payment => {
                            ui.horizontal_top(|ui| {
                                ui.vertical(|ui| {
                                    ui.set_max_width((ui.available_width() - 360.0).max(420.0));
                                    ui.add_enabled_ui(!locked, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label("Pagamento");
                                            if widgets::payment_combo(ui, "doc_pay", &mut self.doc.payment_id, &self.payments, 300.0) {
                                                self.dirty = true;
                                            }
                                        });
                                        ui.add_space(4.0);
                                        self.fiscal_section(ui);
                                    });
                                });
                                self.totals_panel(ui, &totals);
                            });
                        }
                        DocTab::Transport => {
                            ui.add_enabled_ui(!locked, |ui| self.transport_section(ui));
                        }
                        DocTab::Notes => {
                            ui.add_enabled_ui(!locked, |ui| {
                                ui.label("Note riportate in stampa");
                                if ui.add(egui::TextEdit::multiline(&mut self.doc.notes).desired_rows(8).desired_width(f32::INFINITY)).changed() {
                                    self.dirty = true;
                                }
                            });
                        }
                        DocTab::Addresses => {
                            ui.add_enabled_ui(!locked, |ui| {
                                ui.label("Destinazione (se diversa dalla sede)");
                                if ui.add(egui::TextEdit::multiline(&mut self.doc.destination).desired_rows(4).desired_width(520.0)).changed() {
                                    self.dirty = true;
                                }
                            });
                        }
                        DocTab::FeProps => {
                            ui.add_enabled_ui(!locked, |ui| self.tab_fe(ui, cx));
                        }
                        DocTab::Options => {
                            ui.add_enabled_ui(!locked, |ui| self.tab_options(ui, cx));
                        }
                        DocTab::Lines => {}
                    });
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;

    #[test]
    fn listino_del_documento_e_margine() {
        let s = openfatt_store::Store::open_in_memory().unwrap();
        crate::demo::populate(&s).unwrap();
        let parties = s.parties(&openfatt_store::PartyFilter::default()).unwrap();
        let rossi = parties.iter().find(|p| p.name.starts_with("Rossi")).unwrap().clone();
        let sedia = s.product_by_code("SED01").unwrap().unwrap();
        let mut d = Document::new(DocKind::Quote);
        d.party_id = Some(rossi.id);
        d.party = rossi;
        let mut e = Editor::new(&s, d);
        let l = e.line_from_product(&s, &sedia);
        assert_eq!(l.unit_cost, sedia.cost);
        assert_eq!(l.unit_price, dec!(126.7), "listino del cliente");
        e.doc.lines.push(l);
        // listino scelto nel documento: prezzo base
        e.doc.price_list_id = Some(0);
        e.reprice_all(&s);
        assert_eq!(e.doc.lines[0].unit_price, sedia.price);
        // margine: ricavo meno prezzo di acquisto
        let t = calc::compute(&e.doc).unwrap();
        let (rev, cost, missing) = e.margin_totals(&t);
        assert_eq!(rev, t.lines[0].total);
        assert_eq!(cost, sedia.cost);
        assert_eq!(missing, 0);
        // riga senza prezzo di acquisto: non falsa il margine
        e.doc.lines.push(DocLine { description: "Manuale".into(), unit_price: dec!(50), vat: e.doc.lines[0].vat.clone(), ..Default::default() });
        let t = calc::compute(&e.doc).unwrap();
        let (_, cost2, missing) = e.margin_totals(&t);
        assert_eq!((cost2, missing), (sedia.cost, 1));
        // passaggio a prezzi ivati e ritorno: il totale non cambia
        let before = t.total;
        e.set_price_mode(true);
        assert_eq!(calc::compute(&e.doc).unwrap().total, before);
        e.set_price_mode(false);
        assert_eq!(calc::compute(&e.doc).unwrap().total, before);
    }

    #[test]
    fn prices_follow_lists_and_conditions() {
        let s = openfatt_store::Store::open_in_memory().unwrap();
        crate::demo::populate(&s).unwrap();
        let parties = s.parties(&openfatt_store::PartyFilter::default()).unwrap();
        let rossi = parties.iter().find(|p| p.name.starts_with("Rossi")).unwrap().clone();
        let bianchi = parties.iter().find(|p| p.last_name == "Bianchi").unwrap().clone();
        let sedia = s.product_by_code("SED01").unwrap().unwrap();
        let lampada = s.product_by_code("LAM01").unwrap().unwrap();
        let mut d = Document::new(DocKind::Invoice);
        d.party_id = Some(rossi.id);
        d.party = rossi.clone();
        let mut e = Editor::new(&s, d);
        // listino Rivenditori: 149 -15% = 126,65 -> arrotondato 126,70
        let l = e.line_from_product(&s, &sedia);
        assert_eq!(l.unit_price, dec!(126.7));
        assert!(l.price_note.contains("Rivenditori"));
        // scaglione da 10 pezzi: 119
        let (p10, _, note) = e.priced(&s, &sedia, dec!(10), dec!(22));
        assert_eq!(p10, dec!(119));
        assert!(note.contains("da 10"));
        // cambio cliente su bozza: i prezzi seguono le condizioni del nuovo soggetto
        e.doc.lines.push(e.line_from_product(&s, &lampada));
        let mut msgs = crate::ui::Msgs::default();
        e.set_party(&s, &mut msgs, bianchi);
        assert_eq!(e.doc.lines[0].unit_price, dec!(39));
        assert_eq!(e.doc.lines[0].price_note, "Condizione particolare");
        // prezzi IVA inclusa: conversione dal netto
        e.doc.prices_include_vat = true;
        e.reprice_all(&s);
        assert_eq!(e.doc.lines[0].unit_price, dec!(47.58));
    }
}
