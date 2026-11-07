//! Vendita al banco: cassa, vendite del giorno, corrispettivi e impostazioni del registratore.

use std::sync::mpsc::{channel, Receiver};

use chrono::NaiveDate;
use eframe::egui::{self, RichText};
use openfatt_core::calc;
use openfatt_core::model::*;
use openfatt_core::money::{fmt_it_trim, round2};
use openfatt_pos::{Driver, PayKind, Receipt, ReceiptLine, ReceiptPayment, RtConfig, RtOutcome};
use rust_decimal::Decimal;

use super::widgets::{self, eur, GRAY, GREEN, ORANGE, RED};
use super::{Ctx, Nav};

#[derive(PartialEq, Clone, Copy, Default)]
enum Tab {
    #[default]
    Till,
    Sales,
    Summary,
}

pub struct State {
    tab: Tab,
    cart: Vec<DocLine>,
    code: String,
    party: Option<Party>,
    customer_tax_code: String,
    lottery_code: String,
    cash_given: Decimal,
    electronic: Decimal,
    refund: bool,
    refund_ref: RetailRef,
    last: Option<(String, Decimal)>,
    pending: Option<(i64, Receiver<Result<RtOutcome, String>>)>,
    manual: Option<(i64, String, String, String)>,
    confirm_cancel: Option<i64>,
    from: NaiveDate,
    to: NaiveDate,
    focus_code: bool,
    /// Carta fedeltà o buono regalo letto al banco
    card_code: String,
    gift_use: Decimal,
    touch: bool,
}

impl Default for State {
    fn default() -> Self {
        let today = chrono::Local::now().date_naive();
        State {
            tab: Tab::Till,
            cart: Vec::new(),
            code: String::new(),
            party: None,
            customer_tax_code: String::new(),
            lottery_code: String::new(),
            cash_given: Decimal::ZERO,
            electronic: Decimal::ZERO,
            refund: false,
            refund_ref: RetailRef::default(),
            last: None,
            pending: None,
            manual: None,
            confirm_cancel: None,
            from: today,
            to: today,
            focus_code: true,
            card_code: String::new(),
            gift_use: Decimal::ZERO,
            touch: false,
        }
    }
}

impl State {
    /// Stato con una scheda già aperta: "vendite" o "corrispettivi".
    pub fn on_tab(name: &str) -> Self {
        let tab = match name {
            "vendite" => Tab::Sales,
            "corrispettivi" => Tab::Summary,
            _ => Tab::Till,
        };
        State { tab, ..Default::default() }
    }
}

pub fn rt_config(store: &openfatt_store::Store) -> RtConfig {
    store.get_setting("rt").unwrap_or_default()
}

fn gross_price(p: &Product, vats: &[VatCode]) -> (Decimal, VatRef) {
    let v = p.vat_id.and_then(|id| vats.iter().find(|v| v.id == id)).map(VatRef::from_code).unwrap_or(VatRef { vat_id: None, rate: Decimal::from(22), natura: None, legal_ref: String::new() });
    (round2(p.price * (Decimal::ONE + v.rate / Decimal::ONE_HUNDRED)), v)
}

fn add_product(st: &mut State, p: &Product, vats: &[VatCode]) {
    add_product_variant(st, p, "", vats)
}

/// Prezzo IVA inclusa e sconti per il cliente della vendita (listino e condizioni).
fn retail_price(store: &openfatt_store::Store, party: Option<&Party>, p: &Product, qty: Decimal, rate: Decimal) -> Option<(Decimal, String, String)> {
    let party = party?;
    let q = store.quote(Some(party), p, qty, chrono::Local::now().date_naive(), false).ok()?;
    Some((round2(q.price * (Decimal::ONE + rate / Decimal::ONE_HUNDRED)), q.discounts, q.source))
}

/// Ricalcola i prezzi del carrello dopo la scelta del cliente.
fn reprice_cart(store: &openfatt_store::Store, st: &mut State) {
    let vats = store.vat_codes().unwrap_or_default();
    for l in st.cart.iter_mut() {
        let Some(pid) = l.product_id else { continue };
        let Some(p) = store.product(pid).ok().flatten() else { continue };
        match retail_price(store, st.party.as_ref(), &p, l.qty, l.vat.rate) {
            Some((g, d, n)) => {
                l.unit_price = g;
                l.discounts = d;
                l.price_note = n;
            }
            None => {
                l.unit_price = gross_price(&p, &vats).0;
                l.discounts.clear();
            }
        }
    }
}

fn add_product_variant(st: &mut State, p: &Product, variant: &str, vats: &[VatCode]) {
    if p.tracking != Tracking::Serial {
        if let Some(l) = st.cart.iter_mut().rev().find(|l| l.product_id == Some(p.id) && l.discounts.is_empty() && l.variant == variant) {
            l.qty += Decimal::ONE;
            return;
        }
    }
    let (gross, vat) = gross_price(p, vats);
    st.cart.push(DocLine {
        product_id: Some(p.id),
        code: p.code.clone(),
        description: p.description.clone(),
        qty: Decimal::ONE,
        unit: p.unit.clone(),
        unit_price: gross,
        vat,
        unit_cost: p.cost,
        variant: variant.to_string(),
        ..Default::default()
    });
}

/// Costruisce il documento della vendita dal carrello.
fn build_doc(st: &State) -> Document {
    let mut d = Document::new(DocKind::RetailSale);
    d.prices_include_vat = true;
    d.lines = st.cart.clone();
    if st.refund {
        for l in d.lines.iter_mut() {
            l.qty = -l.qty.abs();
        }
    }
    if let Some(p) = &st.party {
        d.party_id = Some(p.id);
        d.party = p.clone();
    }
    d
}

/// Dati per il registratore a partire dalla vendita registrata.
pub fn receipt_from(doc: &Document, cfg: &RtConfig) -> Receipt {
    let info = doc.retail.clone().unwrap_or_default();
    let mut lines = Vec::new();
    for l in doc.lines.iter().filter(|l| l.kind == LineKind::Item) {
        let qty = l.qty.abs();
        let discounts = calc::parse_discounts(&l.discounts).unwrap_or_default();
        let full = round2(l.unit_price * qty);
        let net = round2(calc::apply_cascade(l.unit_price, &discounts) * qty);
        lines.push(ReceiptLine {
            description: l.description.clone(),
            qty,
            unit_price: l.unit_price,
            discount: full - net,
            discount_label: if l.discounts.is_empty() { String::new() } else { format!("SC {}%", l.discounts) },
            vat_rate: l.vat.rate,
            natura: l.vat.natura.clone(),
            department: cfg.department_for(l.vat.rate, l.vat.natura.as_deref()),
            refund: l.qty.is_sign_negative(),
        });
    }
    let mut payments = Vec::new();
    for p in &info.payments {
        let (kind, mut amount) = match p.method {
            RetailPayMethod::Cash => (PayKind::Cash, p.amount),
            RetailPayMethod::Electronic => (PayKind::Electronic, p.amount),
            RetailPayMethod::Ticket => (PayKind::Ticket, p.amount),
            RetailPayMethod::NotCollected => (PayKind::NotCollected, p.amount),
        };
        // il registratore calcola il resto sui contanti ricevuti
        if kind == PayKind::Cash && info.cash_given > amount {
            amount = info.cash_given;
        }
        payments.push(ReceiptPayment { kind, description: p.method.label().to_string(), amount: amount.abs() });
    }
    let total = calc::compute(doc).map(|t| t.total).unwrap_or_default();
    Receipt {
        lines,
        payments,
        total: total.abs(),
        change: info.change,
        customer_tax_code: info.customer_tax_code.clone(),
        lottery_code: info.lottery_code.clone(),
        courtesy_message: cfg.courtesy_message.clone(),
        refund_of: info.refund_of.as_ref().map(|r| (r.rt_serial.clone(), r.z_number.clone(), r.receipt_number.clone(), r.date.map(|d| d.format("%d-%m-%Y").to_string()).unwrap_or_default())),
        operator: cfg.operator,
    }
}

fn start_emission(cx: &mut Ctx, st: &mut State, doc: &Document) {
    let cfg = rt_config(cx.store);
    if matches!(cfg.driver, Driver::Manual) {
        st.manual = Some((doc.id, String::new(), String::new(), String::new()));
        return;
    }
    let receipt = receipt_from(doc, &cfg);
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(openfatt_pos::emit(&cfg, &receipt).map_err(|e| e.0));
    });
    st.pending = Some((doc.id, rx));
}

fn poll_emission(ui: &egui::Ui, cx: &mut Ctx, st: &mut State) {
    let Some((id, rx)) = &st.pending else { return };
    match rx.try_recv() {
        Ok(result) => {
            let id = *id;
            st.pending = None;
            match result {
                Ok(o) => {
                    let status = if o.simulated { RtStatus::Simulated } else { RtStatus::Printed };
                    let r = cx.store.set_retail_outcome(id, status, &o.serial, &o.z_number, &o.receipt_number, &o.message);
                    if cx.msgs.check(r, "").is_some() {
                        if o.simulated {
                            cx.msgs.warn(format!("Simulazione: {} (nessun documento fiscale emesso)", o.message));
                        } else {
                            cx.msgs.ok(format!("Documento commerciale emesso. {}", o.message));
                        }
                    }
                }
                Err(e) => {
                    let _ = cx.store.set_retail_outcome(id, RtStatus::Failed, "", "", "", &e);
                    cx.msgs.err(format!("Registratore: {}. La vendita è registrata: riprovare da 'Vendite' o inserire gli estremi a mano.", e));
                }
            }
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }
        Err(_) => st.pending = None,
    }
}

fn windows(ui: &egui::Ui, cx: &mut Ctx, st: &mut State) {
    let mut close = false;
    if let Some((id, serial, z, n)) = &mut st.manual {
        egui::Window::new("Estremi del documento commerciale").collapsible(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
            ui.label("Battere la vendita sul registratore telematico e riportare i dati stampati sul documento commerciale.");
            egui::Grid::new("manual_rt").num_columns(2).show(ui, |ui| {
                ui.label("Matricola RT");
                ui.text_edit_singleline(serial);
                ui.end_row();
                ui.label("N. chiusura (Z)");
                ui.text_edit_singleline(z);
                ui.end_row();
                ui.label("N. documento");
                ui.text_edit_singleline(n);
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if ui.button("Registra").clicked() {
                    if n.trim().is_empty() {
                        cx.msgs.warn("Indicare almeno il numero del documento");
                    } else if cx.msgs.check(cx.store.set_retail_outcome(*id, RtStatus::Manual, serial, z, n, "Inserito manualmente"), "Estremi registrati").is_some() {
                        close = true;
                    }
                }
                if ui.button("Più tardi").clicked() {
                    close = true;
                }
            });
        });
    }
    if close {
        st.manual = None;
    }
    if let Some(id) = st.confirm_cancel {
        match widgets::confirm(ui.ctx(), "Annulla vendita", "Annullare la vendita? Merce e incasso vengono stornati. Possibile solo se il documento commerciale non è stato emesso.") {
            Some(true) => {
                cx.msgs.check(cx.store.cancel_retail_sale(id), "Vendita annullata");
                st.confirm_cancel = None;
            }
            Some(false) => st.confirm_cancel = None,
            None => {}
        }
    }
}

fn till(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let vats = cx.store.vat_codes().unwrap_or_default();
    let mut doc = build_doc(st);
    let totals = calc::compute(&doc);
    let total = totals.as_ref().map(|t| t.total).unwrap_or_default();
    let due = total.abs();

    ui.columns(2, |cols| {
        // --- carrello ---
        let ui = &mut cols[0];
        ui.horizontal(|ui| {
            ui.label(RichText::new("Codice / barcode").strong());
            let r = ui.add(egui::TextEdit::singleline(&mut st.code).desired_width(220.0).hint_text("leggi o digita e premi Invio").font(egui::TextStyle::Heading));
            if st.focus_code {
                r.request_focus();
                st.focus_code = false;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !st.code.trim().is_empty() {
                // "3*codice" = quantità 3
                let (qty, code) = match st.code.split_once('*') {
                    Some((q, c)) => (openfatt_core::money::parse_dec(q).unwrap_or(Decimal::ONE), c.trim().to_string()),
                    None => (Decimal::ONE, st.code.trim().to_string()),
                };
                match cx.store.find_by_code(&code) {
                    Ok(Some((p, variant))) => {
                        add_product_variant(st, &p, &variant, &vats);
                        if st.party.is_some() {
                            reprice_cart(cx.store, st);
                        }
                        if qty != Decimal::ONE {
                            if let Some(l) = st.cart.last_mut() {
                                l.qty = l.qty - Decimal::ONE + qty;
                            }
                        }
                    }
                    _ => cx.msgs.warn(format!("Articolo '{}' non trovato", code)),
                }
                st.code.clear();
                st.focus_code = true;
            }
        });
        ui.horizontal(|ui| {
            let search_id = ui.make_persistent_id("pos_search");
            let mut search: String = ui.data_mut(|d| d.get_temp(search_id)).unwrap_or_default();
            let mut picked = None;
            egui::ComboBox::from_id_salt("pos_add").width(300.0).height(420.0).selected_text("\u{E30C} cerca articolo...").show_ui(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut search).hint_text("codice o descrizione").desired_width(280.0)).request_focus();
                for p in cx.store.products_page(&search, false, 60).unwrap_or_default() {
                    let (g, _) = gross_price(&p, &vats);
                    if ui.selectable_label(false, format!("{}  {}  € {}", p.code, p.description, eur(g))).clicked() {
                        picked = Some(p);
                    }
                }
            });
            ui.data_mut(|d| d.insert_temp(search_id, search));
            if let Some(p) = picked {
                add_product(st, &p, &vats);
                if st.party.is_some() {
                    reprice_cart(cx.store, st);
                }
                st.focus_code = true;
            }
            if ui.button("\u{E3D4} Voce libera").clicked() {
                let v = vats.iter().find(|v| v.code == "22").map(VatRef::from_code).unwrap_or_default();
                st.cart.push(DocLine { description: "Vendita".into(), qty: Decimal::ONE, vat: v, ..Default::default() });
            }
            if ui.button("\u{E4A6} Svuota").clicked() {
                *st = State { tab: st.tab, touch: st.touch, ..Default::default() };
            }
            ui.toggle_value(&mut st.touch, format!("{} Tasti", super::theme::icon::HAND_TAP)).on_hover_text("Tasti grandi per schermo touch: un tasto per articolo, per categoria");
        });
        if st.touch {
            touch_keys(ui, cx, st, &vats);
        }
        ui.separator();
        let mut remove = None;
        let sale_wh = cx.store.resolve_warehouse(&rt_config(cx.store).warehouse).ok();
        let products: Vec<Option<Product>> = st.cart.iter().map(|l| l.product_id.and_then(|id| cx.store.product(id).ok().flatten())).collect();
        let refund = st.refund;
        egui::ScrollArea::vertical().max_height(ui.available_height() - 10.0).show(ui, |ui| {
            egui::Grid::new("cart").striped(true).num_columns(8).spacing([6.0, 6.0]).show(ui, |ui| {
                for h in ["Descrizione", "Variante / lotto / serie", "Q.tà", "Prezzo", "Sc.%", "IVA", "Importo", ""] {
                    ui.strong(h);
                }
                ui.end_row();
                for (i, l) in st.cart.iter_mut().enumerate() {
                    ui.add_sized([200.0, 22.0], egui::TextEdit::singleline(&mut l.description));
                    super::tracking::cell(ui, cx.store, products[i].as_ref(), l, i + 1000, refund, sale_wh.as_deref());
                    ui.horizontal(|ui| {
                        if ui.small_button("−").clicked() && l.qty > Decimal::ONE {
                            l.qty -= Decimal::ONE;
                        }
                        widgets::dec_field(ui, ("pq", i), &mut l.qty, 46.0, 3);
                        if ui.small_button("+").clicked() {
                            l.qty += Decimal::ONE;
                        }
                    });
                    widgets::dec_field(ui, ("pp", i), &mut l.unit_price, 70.0, 2);
                    ui.add(egui::TextEdit::singleline(&mut l.discounts).desired_width(44.0));
                    widgets::vat_combo(ui, ("pv", i), &mut l.vat, &vats, 60.0);
                    let lt = round2(calc::apply_cascade(l.unit_price, &calc::parse_discounts(&l.discounts).unwrap_or_default()) * l.qty);
                    ui.label(RichText::new(eur(lt)).strong());
                    if ui.small_button("\u{E4A6}").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
            if st.cart.is_empty() {
                ui.add_space(30.0);
                ui.label(RichText::new("Carrello vuoto: leggere un codice a barre o cercare un articolo").color(GRAY));
            }
        });
        if let Some(i) = remove {
            st.cart.remove(i);
        }

        // --- pagamento ---
        let ui = &mut cols[1];
        egui::Frame::group(ui.style()).inner_margin(14.0).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(if st.refund { "DA RESTITUIRE" } else { "TOTALE" }).strong().color(GRAY));
                ui.label(RichText::new(format!("€ {}", eur(due))).size(44.0).strong().color(if st.refund { ORANGE } else { ui.visuals().text_color() }));
                if let Ok(t) = &totals {
                    for r in &t.vat_rows {
                        ui.label(RichText::new(format!("{}: imponibile {} IVA {}", r.natura.clone().unwrap_or_else(|| format!("{}%", fmt_it_trim(r.rate, 0, 2))), eur(r.taxable.abs()), eur(r.tax.abs()))).small().color(GRAY));
                    }
                } else if let Err(e) = &totals {
                    ui.label(RichText::new(e.to_string()).color(RED));
                }
                ui.separator();
                // carta fedeltà o buono regalo
                let card = if st.card_code.trim().is_empty() { None } else { cx.store.card_by_code(&st.card_code).ok().flatten() };
                ui.horizontal(|ui| {
                    ui.label("Carta / buono");
                    ui.add(egui::TextEdit::singleline(&mut st.card_code).desired_width(150.0).hint_text("leggi la carta"));
                    match &card {
                        Some(c) => {
                            let (pts, credit) = cx.store.card_balance(c.id).unwrap_or_default();
                            if c.kind == openfatt_store::CardKind::Gift {
                                ui.label(RichText::new(format!("buono: credito € {}", eur(credit))).color(GREEN));
                                ui.label("usa €");
                                widgets::dec_field(ui, "gift_use", &mut st.gift_use, 70.0, 2);
                                if st.gift_use > credit {
                                    st.gift_use = credit;
                                }
                                if st.gift_use > due {
                                    st.gift_use = due;
                                }
                            } else {
                                ui.label(RichText::new(format!("{} punti, +{} con questa spesa", fmt_it_trim(pts, 0, 0), fmt_it_trim(cx.store.loyalty_points_for(due).unwrap_or_default(), 0, 0))).color(GREEN));
                            }
                        }
                        None if !st.card_code.trim().is_empty() => {
                            ui.label(RichText::new("carta non trovata").color(RED));
                        }
                        None => {}
                    }
                });
                let gift = if card.as_ref().map(|c| c.kind == openfatt_store::CardKind::Gift).unwrap_or(false) { st.gift_use.min(due) } else { Decimal::ZERO };
                let due_before_gift = due;
                let due = due - gift;
                egui::Grid::new("pay").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                    ui.label("Pagamento elettronico");
                    ui.horizontal(|ui| {
                        widgets::dec_field(ui, "pos_el", &mut st.electronic, 110.0, 2);
                        if ui.button("\u{E1D2} tutto").clicked() {
                            st.electronic = due;
                            st.cash_given = Decimal::ZERO;
                        }
                    });
                    ui.end_row();
                    ui.label("Contanti ricevuti");
                    ui.horizontal(|ui| {
                        widgets::dec_field(ui, "pos_cash", &mut st.cash_given, 110.0, 2);
                        if ui.button("\u{E554} importo esatto").clicked() {
                            st.electronic = Decimal::ZERO;
                            st.cash_given = due;
                        }
                    });
                    ui.end_row();
                    ui.label("");
                    ui.horizontal(|ui| {
                        for v in [5, 10, 20, 50, 100] {
                            if ui.small_button(format!("{} €", v)).clicked() {
                                st.cash_given = Decimal::from(v);
                            }
                        }
                    });
                    ui.end_row();
                });
                let cash_part = (due - st.electronic.min(due)).max(Decimal::ZERO);
                let change = if st.cash_given > cash_part { st.cash_given - cash_part } else { Decimal::ZERO };
                let missing = if st.cash_given.is_zero() { Decimal::ZERO } else { (cash_part - st.cash_given).max(Decimal::ZERO) };
                if missing > Decimal::ZERO {
                    ui.label(RichText::new(format!("Mancano € {}", eur(missing))).size(20.0).color(RED));
                } else {
                    ui.label(RichText::new(format!("Resto € {}", eur(change))).size(26.0).strong().color(GREEN));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Cliente");
                    let current = st.party.as_ref().map(|p| p.display_name()).unwrap_or_default();
                    if let Some(p) = widgets::party_picker(ui, cx.store, "pos_party", &current, Some(PartyRole::Customer), 200.0) {
                        if st.customer_tax_code.is_empty() {
                            st.customer_tax_code = if p.tax_code.is_empty() { p.vat_number.clone() } else { p.tax_code.clone() };
                        }
                        st.party = Some(p);
                        reprice_cart(cx.store, st);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("C.F./P.IVA sul documento");
                    ui.add(egui::TextEdit::singleline(&mut st.customer_tax_code).desired_width(170.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Codice lotteria");
                    ui.add(egui::TextEdit::singleline(&mut st.lottery_code).desired_width(120.0));
                });
                ui.checkbox(&mut st.refund, "Reso (restituzione merce)");
                if st.refund {
                    egui::Grid::new("refund_ref").num_columns(2).show(ui, |ui| {
                        ui.label("Documento originale: matricola RT");
                        ui.text_edit_singleline(&mut st.refund_ref.rt_serial);
                        ui.end_row();
                        ui.label("N. chiusura / n. documento");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut st.refund_ref.z_number).desired_width(60.0));
                            ui.add(egui::TextEdit::singleline(&mut st.refund_ref.receipt_number).desired_width(60.0));
                        });
                        ui.end_row();
                        ui.label("Data");
                        let mut d = st.refund_ref.date;
                        widgets::opt_date_field(ui, "refund_date", &mut d);
                        st.refund_ref.date = d;
                        ui.end_row();
                    });
                }
                ui.add_space(8.0);
                let busy = st.pending.is_some();
                let can_close = !st.cart.is_empty() && totals.is_ok() && missing.is_zero() && !busy && !due_before_gift.is_zero();
                let close_key = ui.input(|i| i.key_pressed(egui::Key::F12));
                let btn = ui.add_enabled(can_close, egui::Button::new(RichText::new("\u{E182} Chiudi vendita (F12)").size(22.0)).min_size(egui::vec2(260.0, 52.0)));
                if btn.clicked() || (close_key && can_close) {
                    // pagamenti: elettronico fino al totale, il resto in contanti
                    let sign = if st.refund { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
                    let el = st.electronic.min(due);
                    let mut payments = Vec::new();
                    if !el.is_zero() {
                        payments.push(RetailPayment { method: RetailPayMethod::Electronic, amount: el * sign });
                    }
                    if !cash_part.is_zero() {
                        payments.push(RetailPayment { method: RetailPayMethod::Cash, amount: cash_part * sign });
                    }
                    // buono multiuso speso: pagamento con buoni sul documento commerciale
                    if !gift.is_zero() {
                        payments.push(RetailPayment { method: RetailPayMethod::Ticket, amount: gift * sign });
                    }
                    doc.retail = Some(RetailInfo {
                        payments,
                        cash_given: if st.cash_given.is_zero() { cash_part } else { st.cash_given },
                        change,
                        customer_tax_code: st.customer_tax_code.trim().to_uppercase(),
                        lottery_code: st.lottery_code.trim().to_uppercase(),
                        refund_of: if st.refund { Some(st.refund_ref.clone()) } else { None },
                        ..Default::default()
                    });
                    let cfg = rt_config(cx.store);
                    doc.warehouse = cfg.warehouse.clone();
                    match cx.store.record_retail_sale(&doc, &cfg.cash_account, &cfg.electronic_account) {
                        Ok(saved) => {
                            if let Some(c) = &card {
                                let today = chrono::Local::now().date_naive();
                                let r = if c.kind == openfatt_store::CardKind::Gift {
                                    if gift.is_zero() { Ok(0) } else { cx.store.add_card_move(c.id, today, Decimal::ZERO, -gift * sign, Some(saved.id), &format!("Vendita {}", saved.number_label())) }
                                } else {
                                    let pts = cx.store.loyalty_points_for(due_before_gift).unwrap_or_default() * sign;
                                    cx.store.add_card_move(c.id, today, pts, Decimal::ZERO, Some(saved.id), &format!("Vendita {}", saved.number_label()))
                                };
                                if let Err(e) = r {
                                    cx.msgs.err(e.to_string());
                                }
                            }
                            st.last = Some((saved.number_label(), change));
                            start_emission(cx, st, &saved);
                            let keep_tab = st.tab;
                            let pending = st.pending.take();
                            let manual = st.manual.take();
                            let last = st.last.take();
                            *st = State { tab: keep_tab, pending, manual, last, ..Default::default() };
                        }
                        Err(e) => cx.msgs.err(e.to_string()),
                    }
                }
                if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Invio al registratore...");
                    });
                }
                if let Some((n, ch)) = &st.last {
                    ui.label(RichText::new(format!("Ultima vendita n. {} · resto € {}", n, eur(*ch))).color(GRAY));
                }
            });
        });
    });
}

fn sales(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    ui.horizontal(|ui| {
        ui.label("Dal");
        widgets::date_field(ui, "pos_from", &mut st.from);
        ui.label("al");
        widgets::date_field(ui, "pos_to", &mut st.to);
    });
    let list = cx.store.retail_sales(st.from, st.to).unwrap_or_default();
    let busy = st.pending.is_some();
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("pos_sales").striped(true).num_columns(7).spacing([10.0, 6.0]).show(ui, |ui| {
            for h in ["N.", "Data e ora", "Totale", "Pagamento", "Registratore", "Estremi", ""] {
                ui.strong(h);
            }
            ui.end_row();
            for d in &list {
                let total = calc::compute(d).map(|t| t.total).unwrap_or_default();
                let r = d.retail.clone().unwrap_or_default();
                ui.label(d.number_label());
                ui.label(d.created_at.map(|t| t.format("%d/%m %H:%M").to_string()).unwrap_or_else(|| d.date.format("%d/%m/%Y").to_string()));
                ui.label(RichText::new(eur(total)).strong());
                ui.label(r.payments.iter().map(|p| format!("{} {}", p.method.label(), eur(p.amount))).collect::<Vec<_>>().join(" + "));
                let (txt, col) = if d.status == DocStatus::Cancelled {
                    ("Annullata".to_string(), GRAY)
                } else {
                    let c = match r.rt_status {
                        RtStatus::Printed | RtStatus::Manual => GREEN,
                        RtStatus::Simulated => ORANGE,
                        _ => RED,
                    };
                    (r.rt_status.label().to_string(), c)
                };
                ui.label(RichText::new(txt).color(col)).on_hover_text(&r.rt_message);
                ui.label(format!("{} {}", r.rt_serial, r.reference()));
                ui.horizontal(|ui| {
                    if d.status != DocStatus::Cancelled && matches!(r.rt_status, RtStatus::Pending | RtStatus::Failed) {
                        if ui.add_enabled(!busy, egui::Button::new("Emetti")).clicked() {
                            start_emission(cx, st, d);
                        }
                        if ui.small_button("Estremi a mano").clicked() {
                            st.manual = Some((d.id, String::new(), String::new(), String::new()));
                        }
                        if ui.small_button("Annulla").clicked() {
                            st.confirm_cancel = Some(d.id);
                        }
                    }
                    if d.status != DocStatus::Cancelled && matches!(r.rt_status, RtStatus::Printed | RtStatus::Manual) && total > Decimal::ZERO && ui.small_button("Reso").clicked() {
                        st.cart = d.lines.iter().filter(|l| l.kind == LineKind::Item).cloned().map(|mut l| {
                            l.id = 0;
                            l
                        }).collect();
                        st.refund = true;
                        st.refund_ref = RetailRef { rt_serial: r.rt_serial.clone(), z_number: r.z_number.clone(), receipt_number: r.receipt_number.clone(), date: Some(d.date) };
                        st.tab = Tab::Till;
                    }
                    if ui.small_button("Apri").clicked() {
                        cx.nav.push(Nav::OpenDocument(d.id));
                    }
                });
                ui.end_row();
            }
        });
        if list.is_empty() {
            ui.label(RichText::new("Nessuna vendita nel periodo").color(GRAY));
        }
    });
}

fn summary(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    ui.horizontal(|ui| {
        ui.label("Dal");
        widgets::date_field(ui, "sum_from", &mut st.from);
        ui.label("al");
        widgets::date_field(ui, "sum_to", &mut st.to);
        if ui.button("Questo mese").clicked() {
            let t = chrono::Local::now().date_naive();
            st.from = NaiveDate::from_ymd_opt(chrono::Datelike::year(&t), chrono::Datelike::month(&t), 1).unwrap();
            st.to = t;
        }
        let cfg = rt_config(cx.store);
        if ui.button("\u{E3EC} Chiusura giornaliera del registratore").on_hover_text("Rapporto Z: il registratore trasmette i corrispettivi all'Agenzia delle Entrate").clicked() {
            match openfatt_pos::daily_closure(&cfg) {
                Ok(m) => cx.msgs.ok(m),
                Err(e) => cx.msgs.err(e.0),
            }
        }
    });
    ui.label(RichText::new("Riepilogo per il registro dei corrispettivi e la liquidazione IVA. La trasmissione all'Agenzia la esegue il registratore telematico con la chiusura giornaliera.").small().color(GRAY));
    let days = cx.store.retail_summary(st.from, st.to).unwrap_or_default();
    let tot: Decimal = days.iter().map(|d| d.gross).sum();
    ui.label(RichText::new(format!("Totale corrispettivi nel periodo € {}", eur(tot))).strong());
    let mut csv = String::from("Data;Vendite;Totale;Contanti;Elettronico;Altro;Aliquota;Imponibile;Imposta;Lordo\n");
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("pos_sum").striped(true).num_columns(8).spacing([12.0, 6.0]).show(ui, |ui| {
            for h in ["Data", "Vendite", "Totale", "Contanti", "Elettronico", "Aliquota", "Imponibile", "Imposta"] {
                ui.strong(h);
            }
            ui.end_row();
            for d in &days {
                let date = d.date.map(|x| x.format("%d/%m/%Y").to_string()).unwrap_or_default();
                for (i, (k, taxable, tax, gross)) in d.by_rate.iter().enumerate() {
                    if i == 0 {
                        ui.label(RichText::new(&date).strong());
                        ui.label(format!("{}{}", d.count, if d.pending_rt > 0 { format!(" ({} da emettere)", d.pending_rt) } else { String::new() }));
                        ui.label(RichText::new(eur(d.gross)).strong());
                        ui.label(eur(d.cash));
                        ui.label(eur(d.electronic));
                    } else {
                        for _ in 0..5 {
                            ui.label("");
                        }
                    }
                    ui.label(k);
                    ui.label(eur(*taxable));
                    ui.label(eur(*tax));
                    ui.end_row();
                    csv.push_str(&format!("{};{};{};{};{};{};{};{};{};{}\n", date, d.count, d.gross, d.cash, d.electronic, d.other, k, taxable, tax, gross));
                }
            }
        });
    });
    if !days.is_empty() && ui.button("\u{E20C} Esporta CSV").clicked() {
        if let Some(p) = rfd::FileDialog::new().set_file_name(format!("Corrispettivi {} {}.csv", st.from, st.to)).save_file() {
            cx.msgs.check(std::fs::write(p, csv), "Riepilogo esportato");
        }
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    poll_emission(ui, cx, st);
    windows(ui, cx, st);
    ui.horizontal(|ui| {
        super::title(ui, "Vendita al banco");
        ui.add_space(20.0);
        ui.selectable_value(&mut st.tab, Tab::Till, "\u{ED80} Cassa");
        ui.selectable_value(&mut st.tab, Tab::Sales, "\u{E3EC} Vendite");
        ui.selectable_value(&mut st.tab, Tab::Summary, "\u{E150} Corrispettivi");
        let cfg = rt_config(cx.store);
        ui.label(RichText::new(format!("Registratore: {}", cfg.driver_label())).small().color(if matches!(cfg.driver, Driver::Simulator { .. }) { ORANGE } else { GRAY }));
    });
    ui.separator();
    match st.tab {
        Tab::Till => till(ui, cx, st),
        Tab::Sales => sales(ui, cx, st),
        Tab::Summary => summary(ui, cx, st),
    }
}

/// Scheda "Registratore di cassa" delle impostazioni.
pub fn settings_tab(ui: &mut egui::Ui, cx: &mut Ctx, cfg: &mut RtConfig) {
    ui.label("Il documento commerciale e l'invio dei corrispettivi all'Agenzia delle Entrate sono fatti dal registratore telematico omologato: il programma gli trasmette la vendita.");
    ui.add_space(6.0);
    let mut kind = match cfg.driver {
        Driver::Manual => 0,
        Driver::IniAdapter { .. } => 1,
        Driver::EpsonXml { .. } => 2,
        Driver::Simulator { .. } => 3,
        Driver::Template { .. } => 4,
    };
    let before = kind;
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(&mut kind, 0, "Manuale (battuta sul registratore)");
        ui.radio_value(&mut kind, 1, "Adattatore del produttore (file INI)");
        ui.radio_value(&mut kind, 2, "Epson in rete (XML)");
        ui.radio_value(&mut kind, 4, "Comandi a modelli (file o HTTP)");
        ui.radio_value(&mut kind, 3, "Simulatore (prove)");
    });
    if kind != before {
        cfg.driver = match kind {
            0 => Driver::Manual,
            1 => Driver::IniAdapter { exe: String::new(), port: "Ethernet".into(), ip: String::new() },
            2 => Driver::EpsonXml { host: String::new(), port: 80, timeout_ms: 10000 },
            4 => Driver::Template { transport: openfatt_pos::template::Transport::File { path: String::new(), response: String::new(), timeout_ms: 15000 }, templates: Default::default() },
            _ => Driver::Simulator { folder: crate::config::documents_dir().join("Simulatore registratore").display().to_string() },
        };
    }
    match &mut cfg.driver {
        Driver::Manual => {
            ui.label(RichText::new("Dopo ogni vendita il programma chiede gli estremi (matricola, numero chiusura e numero documento) del documento commerciale battuto sul registratore.").color(GRAY));
        }
        Driver::IniAdapter { exe, port, ip } => {
            ui.horizontal(|ui| {
                ui.label("Eseguibile dell'adattatore");
                ui.add(egui::TextEdit::singleline(exe).desired_width(380.0));
                if ui.button("Scegli...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Programma", &["exe", "bat", "cmd"]).pick_file() {
                        *exe = p.display().to_string();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Porta");
                ui.add(egui::TextEdit::singleline(port).desired_width(100.0).hint_text("COM1 o Ethernet"));
                ui.label("Indirizzo IP");
                ui.add(egui::TextEdit::singleline(ip).desired_width(140.0));
            });
            ui.label(RichText::new("Formato di interscambio INI versione 2: il file viene scritto nella cartella dell'adattatore e il suo percorso passato come argomento. Codice di uscita 0 = documento emesso.").small().color(GRAY));
        }
        Driver::EpsonXml { host, port, timeout_ms } => {
            ui.horizontal(|ui| {
                ui.label("Indirizzo IP");
                ui.add(egui::TextEdit::singleline(host).desired_width(160.0));
                ui.label("Porta");
                let mut p = Decimal::from(*port);
                if widgets::dec_field(ui, "rt_port", &mut p, 60.0, 0) {
                    *port = rust_decimal::prelude::ToPrimitive::to_u16(&p).unwrap_or(80);
                }
                ui.label("Attesa massima (ms)");
                let mut t = Decimal::from(*timeout_ms);
                if widgets::dec_field(ui, "rt_to", &mut t, 70.0, 0) {
                    *timeout_ms = rust_decimal::prelude::ToPrimitive::to_u32(&t).unwrap_or(10000);
                }
            });
            ui.label(RichText::new("Protocollo XML su HTTP (cgi-bin/fpmate.cgi) dei registratori telematici Epson. Da collaudare sul proprio modello prima dell'uso in negozio.").small().color(ORANGE));
        }
        Driver::Template { transport, templates } => {
            use openfatt_pos::template::Transport;
            ui.label(RichText::new("Per i registratori che si collegano con un programma del produttore o del rivenditore (Ditron, RCH, Micrelec e altri) o che ricevono comandi via rete. I comandi vanno scritti secondo il manuale tecnico del registratore: il gestionale non li conosce e non li inventa. In alternativa, quasi tutti i produttori forniscono un adattatore per il formato INI.").small().color(ORANGE));
            let mut http = matches!(transport, Transport::Http { .. });
            ui.horizontal(|ui| {
                if ui.radio_value(&mut http, false, "File in una cartella").changed() {
                    *transport = Transport::File { path: String::new(), response: String::new(), timeout_ms: 15000 };
                }
                if ui.radio_value(&mut http, true, "Richiesta HTTP").changed() {
                    *transport = Transport::Http { host: String::new(), port: 80, path: "/".into(), content_type: "text/plain".into(), timeout_ms: 10000 };
                }
            });
            match transport {
                Transport::File { path, response, .. } => {
                    ui.horizontal(|ui| {
                        ui.label("File dei comandi");
                        ui.add(egui::TextEdit::singleline(path).desired_width(360.0).hint_text("C:\\Collegamento\\scontrino.txt"));
                    });
                    ui.horizontal(|ui| {
                        ui.label("File di risposta (facoltativo)");
                        ui.add(egui::TextEdit::singleline(response).desired_width(300.0));
                    });
                }
                Transport::Http { host, port, path, content_type, .. } => {
                    ui.horizontal(|ui| {
                        ui.label("Indirizzo");
                        ui.add(egui::TextEdit::singleline(host).desired_width(140.0));
                        ui.label("Porta");
                        let mut p = Decimal::from(*port);
                        if widgets::dec_field(ui, "tpl_port", &mut p, 60.0, 0) {
                            *port = rust_decimal::prelude::ToPrimitive::to_u16(&p).unwrap_or(80);
                        }
                        ui.label("Percorso");
                        ui.add(egui::TextEdit::singleline(path).desired_width(160.0));
                        ui.label("Tipo");
                        ui.add(egui::TextEdit::singleline(content_type).desired_width(120.0));
                    });
                }
            }
            egui::Grid::new("tpl").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                for (label, v) in [("Apertura", &mut templates.begin), ("Riga", &mut templates.line), ("Sconto di riga", &mut templates.discount), ("Pagamento", &mut templates.payment), ("Chiusura documento", &mut templates.end), ("Chiusura giornaliera", &mut templates.closure), ("Segnale di errore", &mut templates.error_marker)] {
                    ui.label(label);
                    ui.add(egui::TextEdit::singleline(v).desired_width(460.0).font(egui::TextStyle::Monospace));
                    ui.end_row();
                }
                ui.label("Codici pagamento");
                ui.horizontal(|ui| {
                    for (i, l) in ["contanti", "elettronico", "buoni", "non riscosso"].iter().enumerate() {
                        ui.label(*l);
                        ui.add(egui::TextEdit::singleline(&mut templates.tenders[i]).desired_width(40.0));
                    }
                });
                ui.end_row();
                ui.label("Separatore decimale");
                ui.add(egui::TextEdit::singleline(&mut templates.decimal_sep).desired_width(30.0));
                ui.end_row();
            });
            ui.label(RichText::new("Segnaposto: {desc} {qty} {price} {price_cents} {total} {dept} {vat} {disc} {tender} {amount} {amount_cents} {cf} {lottery}").small().color(GRAY));
        }
        Driver::Simulator { folder } => {
            ui.horizontal(|ui| {
                ui.label("Cartella");
                ui.add(egui::TextEdit::singleline(folder).desired_width(420.0));
            });
            ui.label(RichText::new("Il simulatore scrive i file INI e XML che verrebbero inviati e assegna numeri fittizi. NON emette documenti fiscali.").small().color(ORANGE));
        }
    }
    ui.separator();
    ui.label(RichText::new("Reparti del registratore per aliquota").strong());
    let mut remove = None;
    egui::Grid::new("deps").num_columns(3).show(ui, |ui| {
        ui.strong("Aliquota o natura");
        ui.strong("Reparto");
        ui.label("");
        ui.end_row();
        for (i, d) in cfg.departments.iter_mut().enumerate() {
            ui.add(egui::TextEdit::singleline(&mut d.key).desired_width(70.0));
            let mut v = Decimal::from(d.department);
            if widgets::dec_field(ui, ("dep", i), &mut v, 50.0, 0) {
                d.department = rust_decimal::prelude::ToPrimitive::to_u32(&v).unwrap_or(1);
            }
            if ui.small_button("\u{E4A6}").clicked() {
                remove = Some(i);
            }
            ui.end_row();
        }
    });
    if let Some(i) = remove {
        cfg.departments.remove(i);
    }
    if ui.small_button("\u{E3D4} Reparto").clicked() {
        cfg.departments.push(openfatt_pos::DepartmentMap { key: String::new(), department: cfg.departments.len() as u32 + 1 });
    }
    egui::Grid::new("rt_misc").num_columns(2).show(ui, |ui| {
        ui.label("Reparto predefinito");
        let mut v = Decimal::from(cfg.default_department);
        if widgets::dec_field(ui, "dep_def", &mut v, 50.0, 0) {
            cfg.default_department = rust_decimal::prelude::ToPrimitive::to_u32(&v).unwrap_or(1);
        }
        ui.end_row();
        ui.label("Operatore");
        let mut v = Decimal::from(cfg.operator);
        if widgets::dec_field(ui, "rt_op", &mut v, 50.0, 0) {
            cfg.operator = rust_decimal::prelude::ToPrimitive::to_u32(&v).unwrap_or(1);
        }
        ui.end_row();
        ui.label("Messaggio di cortesia");
        ui.add(egui::TextEdit::singleline(&mut cfg.courtesy_message).desired_width(300.0));
        ui.end_row();
        ui.label("Magazzino del punto vendita");
        let whs = cx.store.warehouses().unwrap_or_default();
        widgets::warehouse_combo(ui, "rt_wh", &mut cfg.warehouse, &whs, true, 200.0);
        ui.end_row();
        ui.label("Conto prima nota contanti");
        ui.add(egui::TextEdit::singleline(&mut cfg.cash_account).desired_width(140.0));
        ui.end_row();
        ui.label("Conto prima nota elettronici");
        ui.add(egui::TextEdit::singleline(&mut cfg.electronic_account).desired_width(140.0));
        ui.end_row();
    });
    if ui.button("\u{E248} Salva").clicked() {
        cx.msgs.check(cx.store.set_setting("rt", &*cfg), "Impostazioni del registratore salvate");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;

    #[test]
    fn sale_to_register_end_to_end() {
        let s = openfatt_store::Store::open_in_memory().unwrap();
        crate::demo::populate(&s).unwrap();
        let vats = s.vat_codes().unwrap();
        let sedia = s.product_by_code("SED01").unwrap().unwrap();
        let libro = s.product_by_code("LIB01").unwrap().unwrap();
        let mut st = State::default();
        add_product(&mut st, &sedia, &vats);
        add_product(&mut st, &sedia, &vats); // stessa riga: quantità 2
        add_product(&mut st, &libro, &vats);
        assert_eq!(st.cart.len(), 2);
        assert_eq!(st.cart[0].qty, dec!(2));
        assert_eq!(st.cart[0].unit_price, dec!(181.78)); // 149 + 22%
        st.cart[0].discounts = "10".into();
        let mut doc = build_doc(&st);
        let total = calc::compute(&doc).unwrap().total;
        // 181,78 x 0,9 x 2 = 327,20 ; libro 18 + 10% = 19,80
        assert_eq!(total, dec!(347.00));
        doc.retail = Some(RetailInfo {
            payments: vec![RetailPayment { method: RetailPayMethod::Electronic, amount: dec!(100) }, RetailPayment { method: RetailPayMethod::Cash, amount: dec!(247.00) }],
            cash_given: dec!(250),
            change: dec!(3),
            ..Default::default()
        });
        let saved = s.record_retail_sale(&doc, "Cassa", "POS").unwrap();
        let dir = std::env::temp_dir().join(format!("openfatt-pos-e2e-{}", std::process::id()));
        let cfg = RtConfig { driver: Driver::Simulator { folder: dir.display().to_string() }, ..Default::default() };
        let r = receipt_from(&saved, &cfg);
        // il totale inviato al registratore coincide con quello registrato
        let sum: Decimal = r.lines.iter().map(|l| l.total()).sum();
        assert_eq!(sum, dec!(347.00));
        assert_eq!(r.lines[0].discount, dec!(36.36));
        assert_eq!(r.lines[0].department, 1);
        assert_eq!(r.lines[1].department, 2);
        // contanti: al registratore si passano i 250 ricevuti (calcola il resto)
        assert!(r.payments.iter().any(|p| p.kind == PayKind::Cash && p.amount == dec!(250)));
        let ini = openfatt_pos::ini::to_ini(&r, "Ethernet", "");
        assert!(ini.contains("TotScontrino=347.00"));
        assert!(ini.contains("TotResto=3.00"));
        let o = openfatt_pos::emit(&cfg, &r).unwrap();
        let done = s.set_retail_outcome(saved.id, RtStatus::Simulated, &o.serial, &o.z_number, &o.receipt_number, &o.message).unwrap();
        assert_eq!(done.retail.unwrap().rt_status, RtStatus::Simulated);
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Tastiera per schermi touch: categorie in alto, un tasto grande per articolo.
fn touch_keys(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, vats: &[VatCode]) {
    let id = ui.make_persistent_id("touch_cat");
    let mut cat: String = ui.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    let products: Vec<Product> = cx.store.products_page("", false, 2000).unwrap_or_default().into_iter().filter(|p| p.price > Decimal::ZERO).collect();
    let mut cats: Vec<String> = products.iter().map(|p| p.category.split('>').next().unwrap_or("").trim().to_string()).filter(|c| !c.is_empty()).collect();
    cats.sort();
    cats.dedup();
    if cat.is_empty() {
        cat = cats.first().cloned().unwrap_or_default();
    }
    ui.horizontal_wrapped(|ui| {
        for c in &cats {
            let sel = *c == cat;
            if ui.add(egui::Button::new(RichText::new(c).size(16.0)).selected(sel).min_size(egui::vec2(96.0, 40.0))).clicked() {
                cat = c.clone();
            }
        }
    });
    egui::ScrollArea::vertical().id_salt("touch_keys").max_height(230.0).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for p in products.iter().filter(|p| cats.is_empty() || p.category.split('>').next().unwrap_or("").trim() == cat).take(60) {
                let (g, _) = gross_price(p, vats);
                let label = format!("{}\n€ {}", super::widgets::truncate(&p.description, 22), eur(g));
                if ui.add(egui::Button::new(RichText::new(label).size(15.0)).min_size(egui::vec2(132.0, 64.0)).wrap()).clicked() {
                    if p.variants.is_empty() {
                        add_product(st, p, vats);
                        if st.party.is_some() {
                            reprice_cart(cx.store, st);
                        }
                    } else {
                        cx.msgs.warn("Articolo con varianti: leggere il codice a barre della variante");
                    }
                }
            }
        });
    });
    ui.data_mut(|d| d.insert_temp(id, cat));
}
