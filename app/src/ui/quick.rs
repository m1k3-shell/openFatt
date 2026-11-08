//! Schede rapide: cliccando un cliente, un fornitore o un articolo in qualunque punto del
//! programma si apre una finestra con tutti i dettagli, senza lasciare il lavoro in corso.
//! Da qui passano anche i movimenti manuali di magazzino (carico, scarico, rettifica).

use std::collections::HashMap;

use chrono::NaiveDate;
use eframe::egui::{self, RichText};
use openfatt_core::codes;
use openfatt_core::model::{Party, Product, StockMove, Tracking};
use openfatt_core::money::fmt_it_trim;
use openfatt_store::{DocFilter, DocSummary, DueRow, StockInfo, StockState};
use rust_decimal::Decimal;

use super::grid::{Col, Grid, Val};
use super::theme::{self, icon, Tone};
use super::widgets::{self, eur, ORANGE, RED};
use super::{Ctx, Nav};

/// Elemento di cui aprire la scheda.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ref {
    Party(i64),
    Product(i64),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MoveKind {
    Load,
    Unload,
    Adjust,
}

#[derive(Clone, Copy)]
enum Request {
    Card(Ref),
    Moves(i64),
    Move(MoveKind, i64),
}

fn request_id() -> egui::Id {
    egui::Id::new("openfatt_quick_request")
}

fn send(ctx: &egui::Context, r: Request) {
    ctx.data_mut(|d| d.insert_temp(request_id(), r));
}

/// Apre la scheda dell'elemento.
pub fn open(ctx: &egui::Context, r: Ref) {
    send(ctx, Request::Card(r));
}

/// Apre la finestra di carico, scarico o rettifica dell'articolo.
pub fn open_move(ctx: &egui::Context, kind: MoveKind, product: i64) {
    send(ctx, Request::Move(kind, product));
}

/// Anche nei documenti bloccati le schede si devono poter aprire: i controlli disabilitati non
/// ricevono clic, quindi si guarda direttamente il puntatore.
fn clicked_while_disabled(ui: &egui::Ui, rect: egui::Rect) -> bool {
    if ui.is_enabled() {
        return false;
    }
    let pos = ui.input(|i| if i.pointer.primary_clicked() { i.pointer.interact_pos() } else { None });
    // le schermate di fondo non sono finestre: nessun livello sotto il puntatore vuol dire che si è sul fondo
    pos.is_some_and(|p| rect.contains(p) && ui.clip_rect().contains(p) && ui.ctx().layer_id_at(p).is_none_or(|top| top == ui.layer_id()))
}

/// Testo cliccabile che apre la scheda.
pub fn link(ui: &mut egui::Ui, text: impl Into<String>, r: Ref) -> egui::Response {
    let color = ui.visuals().override_text_color.unwrap_or(theme::ACCENT);
    let resp = ui.add(egui::Label::new(RichText::new(text.into()).color(color)).sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Apri la scheda");
    if resp.clicked() || clicked_while_disabled(ui, resp.rect) {
        open(ui.ctx(), r);
    }
    resp
}

/// Pulsantino che apre la scheda, accanto ai campi modificabili.
pub fn info_button(ui: &mut egui::Ui, r: Ref) -> egui::Response {
    let resp = ui.add(egui::Button::new(RichText::new(icon::INFO).color(theme::ACCENT)).frame(false).min_size(egui::vec2(18.0, 18.0))).on_hover_text("Apri la scheda");
    if resp.clicked() || clicked_while_disabled(ui, resp.rect) {
        open(ui.ctx(), r);
    }
    resp
}

fn kv(ui: &mut egui::Ui, k: &str, v: impl Into<String>) {
    let v = v.into();
    if v.trim().is_empty() {
        return;
    }
    ui.label(theme::muted(k));
    ui.add(egui::Label::new(v).wrap());
    ui.end_row();
}

fn qty(d: Decimal) -> String {
    fmt_it_trim(d, 0, 3)
}

fn date(d: Option<NaiveDate>) -> String {
    d.map(|x| x.format("%d/%m/%Y").to_string()).unwrap_or_else(|| "-".into())
}

fn help(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new("(?)").color(theme::ACCENT)).on_hover_text(text);
}

/// Pulsanti di magazzino e situazione dell'articolo: usati nella scheda rapida e nella scheda laterale.
pub fn stock_status(ui: &mut egui::Ui, p: &Product, i: &StockInfo) {
    if !p.manage_stock {
        ui.label(theme::muted("Per questo articolo il magazzino non è gestito."));
        return;
    }
    ui.columns(3, |c| {
        for (n, (kind, t)) in [(MoveKind::Load, "Carica"), (MoveKind::Unload, "Scarica"), (MoveKind::Adjust, "Rettifica")].into_iter().enumerate() {
            if c[n].add_sized([c[n].available_width(), 26.0], egui::Button::new(t)).clicked() {
                open_move(c[n].ctx(), kind, p.id);
            }
        }
    });
    if ui.link("Mostra movimenti magazzino").clicked() {
        send(ui.ctx(), Request::Moves(p.id));
    }
    ui.add_space(6.0);
    egui::Grid::new(("stock_status", p.id)).num_columns(2).spacing([16.0, 4.0]).min_col_width(130.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(theme::bold("Stato"));
            help(ui, "Regolare: disponibilità sopra la scorta minima. Sotto scorta: disponibile inferiore alla scorta minima. Esaurito: giacenza a zero o negativa.");
        });
        let color = match i.state {
            StockState::Regular => theme::INK,
            StockState::UnderMin => ORANGE,
            StockState::OutOfStock => RED,
        };
        ui.label(theme::bold(i.state.label()).color(color));
        ui.end_row();
        ui.horizontal(|ui| {
            ui.label("Q.tà in giacenza");
            ui.label(theme::muted(format!("({} - {})", qty(i.loaded), qty(i.unloaded)))).on_hover_text("Totale caricato meno totale scaricato");
        });
        ui.label(qty(i.stock));
        ui.end_row();
        ui.label("Q.tà impegnata");
        ui.label(qty(i.committed));
        ui.end_row();
        ui.label(theme::bold("Q.tà disponibile"));
        ui.label(theme::bold(qty(i.available)));
        ui.end_row();
        ui.label("Q.tà in arrivo");
        ui.label(qty(i.incoming));
        ui.end_row();
        ui.label("");
        ui.label("");
        ui.end_row();
        ui.label("Vendita media mensile");
        ui.label(fmt_it_trim(i.monthly_sales, 2, 2));
        ui.end_row();
        ui.horizontal(|ui| {
            ui.label("Stima fine scorte");
            help(ui, "Data in cui, alla velocità media di vendita, il disponibile si azzera.");
        });
        ui.label(date(i.run_out));
        ui.end_row();
        ui.label("Stima prossimo ordine");
        ui.label(date(i.next_order));
        ui.end_row();
        ui.label("");
        ui.label("");
        ui.end_row();
        ui.label("Primo carico");
        ui.label(date(i.first_load));
        ui.end_row();
        ui.label("Ultimo carico");
        ui.label(date(i.last_load));
        ui.end_row();
        ui.label("Ultimo scarico");
        ui.label(date(i.last_unload));
        ui.end_row();
        ui.label("");
        ui.label("");
        ui.end_row();
        let money = |v: Option<Decimal>| v.map(|x| format!("€ {}", eur(x.round_dp(2)))).unwrap_or_else(|| "-".into());
        ui.label("Costo medio");
        ui.label(money(i.avg_cost));
        ui.end_row();
        ui.label("Ultimo costo");
        ui.label(money(i.last_cost));
        ui.end_row();
        ui.label("Prezzo medio vendita");
        ui.label(money(i.avg_sale));
        ui.end_row();
    });
}

struct ProductCard {
    id: i64,
    tab: u8,
    generation: u64,
    product: Option<Product>,
    info: StockInfo,
    by_warehouse: Vec<(String, Decimal)>,
    lists: Vec<(String, Option<Decimal>)>,
    vat_rate: Decimal,
    vat_label: String,
    suppliers: Vec<(i64, String, String, Decimal, String)>,
    moves: Vec<StockMove>,
    grid: Grid,
    photo: Option<egui::TextureHandle>,
}

impl ProductCard {
    fn new(id: i64) -> Self {
        ProductCard { id, tab: 0, generation: u64::MAX, product: None, info: Default::default(), by_warehouse: Vec::new(), lists: Vec::new(), vat_rate: Decimal::ZERO, vat_label: String::new(), suppliers: Vec::new(), moves: Vec::new(), grid: Grid::new("scheda-movimenti"), photo: None }
    }

    fn load(&mut self, ctx: &egui::Context, cx: &Ctx) {
        let generation = cx.store.generation();
        if generation == self.generation {
            return;
        }
        self.generation = generation;
        let today = chrono::Local::now().date_naive();
        let Some(p) = cx.store.product(self.id).ok().flatten() else {
            self.product = None;
            return;
        };
        self.info = cx.store.stock_info(&p, today).unwrap_or_default();
        self.by_warehouse = cx.store.stock_by_warehouse(p.id).unwrap_or_default();
        self.moves = cx.store.stock_moves(Some(p.id), 2000).unwrap_or_default();
        self.lists = cx.store.price_lists().unwrap_or_default().into_iter().filter(|l| l.active).map(|l| (l.name.clone(), cx.store.list_price_of(l.id, &p, Decimal::ZERO).ok().flatten().map(|x| x.0))).collect();
        let vat = p.vat_id.and_then(|id| cx.store.vat_codes().ok()?.into_iter().find(|v| v.id == id));
        self.vat_rate = vat.as_ref().map(|v| v.rate).unwrap_or_default();
        self.vat_label = vat.map(|v| format!("{} - {}", v.code, v.description)).unwrap_or_default();
        let name = |id: i64| cx.store.party(id).ok().flatten().map(|x| x.display_name()).unwrap_or_default();
        self.suppliers.clear();
        if let Some(s) = p.supplier_id {
            self.suppliers.push((s, name(s), p.supplier_code.clone(), p.supplier_price, p.supplier_notes.clone()));
        }
        for s in &p.other_suppliers {
            self.suppliers.push((s.supplier_id, name(s.supplier_id), s.code.clone(), s.price, s.notes.clone()));
        }
        self.photo = cx.store.product_photo(p.id).ok().flatten().and_then(|bytes| {
            let img = image::load_from_memory(&bytes).ok()?.into_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            Some(ctx.load_texture("scheda-foto", egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), Default::default()))
        });
        self.product = Some(p);
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut Ctx) -> bool {
        self.load(ctx, cx);
        let mut open = true;
        let Some(p) = self.product.clone() else {
            cx.msgs.warn("L'articolo non esiste più");
            return false;
        };
        let mut full = false;
        egui::Window::new(format!("{}  {}", p.code, widgets::truncate(&p.description, 60))).id(egui::Id::new("qc_product")).open(&mut open).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.screen_rect().center()).default_size([620.0, 560.0]).min_width(460.0).collapsible(false).show(ctx, |ui| {
            egui::TopBottomPanel::bottom("qc_product_bar").show_inside(ui, |ui| {
                ui.add_space(4.0);
                if theme::button(ui, icon::ARROW_SQUARE_OUT, "Apri scheda completa").clicked() {
                    full = true;
                }
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, t) in ["Prodotto", "Caratteristiche", "Dim. e peso", "Magazzino", "Fornitori", "Movimenti"].iter().enumerate() {
                    ui.selectable_value(&mut self.tab, i as u8, *t);
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().id_salt("qc_product_scroll").auto_shrink([false, false]).show(ui, |ui| match self.tab {
                0 => self.tab_product(ui, cx, &p),
                1 => {
                    egui::Grid::new("qc_features").num_columns(2).spacing([16.0, 5.0]).show(ui, |ui| {
                        kv(ui, "Produttore", p.producer.clone());
                        kv(ui, "Sito web", p.url.clone());
                        kv(ui, "Tipo", p.kind.label());
                        kv(ui, "Tracciabilità", p.tracking.label());
                        kv(ui, "Confezione", if p.pack_size.is_zero() { String::new() } else { format!("{} pz per {}", qty(p.pack_size), p.pack_unit) });
                        kv(ui, "Contributo ambientale", if p.eco_fee.is_zero() { String::new() } else { format!("€ {} {}", eur(p.eco_fee), p.eco_label) });
                        kv(ui, "Provvigione", p.commission.map(|c| format!("{}%", fmt_it_trim(c, 0, 2))).unwrap_or_default());
                        kv(ui, "Note", p.notes.clone());
                    });
                }
                2 => {
                    let triple = |v: [Decimal; 3]| if v.iter().all(|x| x.is_zero()) { String::new() } else { format!("{} x {} x {} {}", qty(v[0]), qty(v[1]), qty(v[2]), p.dim_unit) };
                    egui::Grid::new("qc_dim").num_columns(2).spacing([16.0, 5.0]).show(ui, |ui| {
                        kv(ui, "Dimensioni nette", triple(p.dim_net));
                        kv(ui, "Dimensioni imballo", triple(p.dim_pack));
                        kv(ui, "Peso netto", if p.weight_net.is_zero() { String::new() } else { format!("{} {}", qty(p.weight_net), p.weight_unit) });
                        kv(ui, "Peso lordo", if p.weight_gross.is_zero() { String::new() } else { format!("{} {}", qty(p.weight_gross), p.weight_unit) });
                    });
                }
                3 => {
                    egui::Grid::new("qc_stock_cfg").num_columns(2).spacing([16.0, 5.0]).show(ui, |ui| {
                        kv(ui, "Ubicazione", p.location.clone());
                        kv(ui, "Scorta minima", if p.min_stock.is_zero() { String::new() } else { format!("{} {}", qty(p.min_stock), p.unit) });
                        kv(ui, "Riordino", if p.order_days == 0 { String::new() } else { format!("{} gg.", p.order_days) });
                        kv(ui, "Ordina multipli di", if p.order_multiple.is_zero() { String::new() } else { qty(p.order_multiple) });
                    });
                    ui.add_space(6.0);
                    stock_status(ui, &p, &self.info);
                    if self.by_warehouse.len() > 1 {
                        ui.add_space(8.0);
                        ui.label(theme::medium("Giacenza per magazzino"));
                        egui::Grid::new("qc_wh").num_columns(2).spacing([16.0, 3.0]).show(ui, |ui| {
                            for (w, q) in &self.by_warehouse {
                                ui.label(w);
                                ui.label(qty(*q));
                                ui.end_row();
                            }
                        });
                    }
                }
                4 => {
                    if self.suppliers.is_empty() {
                        ui.label(theme::muted("Nessun fornitore indicato."));
                    }
                    egui::Grid::new("qc_sup").num_columns(4).striped(true).spacing([14.0, 5.0]).show(ui, |ui| {
                        for (id, name, code, price, notes) in &self.suppliers {
                            link(ui, name.clone(), Ref::Party(*id));
                            ui.label(code);
                            ui.label(format!("€ {}", eur(*price)));
                            ui.label(theme::muted(notes));
                            ui.end_row();
                        }
                    });
                }
                _ => {}
            });
            if self.tab == 5 {
                // l'elenco dei movimenti gestisce da sé lo scorrimento
                self.tab_moves(ui, cx);
            }
        });
        if full {
            cx.nav.push(Nav::FindProduct(p.id, p.code.clone()));
            return false;
        }
        open
    }

    fn tab_product(&self, ui: &mut egui::Ui, cx: &Ctx, p: &Product) {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                egui::Grid::new("qc_main").num_columns(2).spacing([16.0, 5.0]).show(ui, |ui| {
                    kv(ui, "Codice", p.code.clone());
                    kv(ui, "Descrizione", p.description.clone());
                    kv(ui, "Categoria", p.category.replace(" > ", " » "));
                    kv(ui, "Codice a barre", p.barcode.clone());
                    kv(ui, "U.M.", p.unit.clone());
                    kv(ui, "Aliquota IVA", self.vat_label.clone());
                    kv(ui, "Tipo", if p.is_service { "Servizio" } else { "Articolo" });
                    kv(ui, "Stato", if p.active { "Attivo" } else { "Non attivo" });
                    kv(ui, "Giacenza", if p.manage_stock { format!("{} {} (disponibili {})", qty(self.info.stock), p.unit, qty(self.info.available)) } else { String::new() });
                });
            });
            if let Some(t) = &self.photo {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.add(egui::Image::new(t).max_size(egui::vec2(150.0, 150.0)));
                });
            }
        });
        ui.add_space(8.0);
        ui.label(theme::medium("Prezzi"));
        let gross = |n: Decimal| (n * (Decimal::ONE + self.vat_rate / Decimal::ONE_HUNDRED)).round_dp(2);
        egui::Grid::new("qc_prices").num_columns(3).striped(true).spacing([20.0, 4.0]).show(ui, |ui| {
            ui.label(theme::muted("Listino"));
            ui.label(theme::muted("Netto"));
            ui.label(theme::muted("Ivato"));
            ui.end_row();
            ui.label(cx.store.base_list_name());
            ui.label(format!("€ {}", eur(p.price)));
            ui.label(format!("€ {}", eur(gross(p.price))));
            ui.end_row();
            for (name, price) in &self.lists {
                ui.label(name);
                match price {
                    Some(n) => {
                        ui.label(format!("€ {}", eur(n.round_dp(2))));
                        ui.label(format!("€ {}", eur(gross(*n))));
                    }
                    None => {
                        ui.label("-");
                        ui.label("-");
                    }
                }
                ui.end_row();
            }
            ui.label("Costo");
            ui.label(format!("€ {}", eur(p.cost)));
            let margin = if p.price.is_zero() || p.cost.is_zero() { String::new() } else { format!("margine {}%", fmt_it_trim((p.price - p.cost) / p.price * Decimal::ONE_HUNDRED, 0, 1)) };
            ui.label(theme::muted(margin));
            ui.end_row();
        });
    }

    fn tab_moves(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let opened = std::cell::Cell::new(None);
        let opened_ref = &opened;
        let cols: Vec<Col<StockMove>> = vec![
            Col::new("date", "Data", 90.0, |m: &StockMove| Val::Date(Some(m.date))),
            Col::new("qty", "Quantità", 80.0, |m: &StockMove| Val::Num(m.qty, 3)).right().total(),
            Col::text("reason", "Causale", 220.0, |m: &StockMove| m.reason.clone()).render(move |ui: &mut egui::Ui, m: &StockMove| match m.document_id {
                Some(d) => {
                    if ui.link(&m.reason).clicked() {
                        opened_ref.set(Some(d));
                    }
                }
                None => {
                    ui.label(&m.reason);
                }
            }),
            Col::text("wh", "Magazzino", 100.0, |m: &StockMove| m.warehouse.clone()).group(),
            Col::text("lot", "Variante / lotto / serie", 150.0, |m: &StockMove| [m.variant.as_str(), m.lot.as_str(), m.serial.as_str()].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" · ")).hidden(),
            Col::money("value", "Valore unit.", 90.0, |m: &StockMove| m.unit_value).hidden(),
        ];
        ui.horizontal(|ui| {
            self.grid.toolbar(ui, cx.store, cx.msgs, &cols, &self.moves, "Movimenti di magazzino");
        });
        let h = ui.available_height() - 40.0;
        self.grid.version = 0;
        self.grid.show(ui, cx.store, &cols, &self.moves, |m| m.id, h, false, 22.0);
        if let Some(d) = opened.get() {
            cx.nav.push(Nav::OpenDocument(d));
        }
    }
}

struct PartyCard {
    id: i64,
    tab: u8,
    generation: u64,
    party: Option<Party>,
    docs: Vec<DocSummary>,
    dues: Vec<DueRow>,
    payment: String,
    list: String,
    agent: String,
    doc_grid: Grid,
    due_grid: Grid,
}

impl PartyCard {
    fn new(id: i64) -> Self {
        PartyCard { id, tab: 0, generation: u64::MAX, party: None, docs: Vec::new(), dues: Vec::new(), payment: String::new(), list: String::new(), agent: String::new(), doc_grid: Grid::new("scheda-documenti"), due_grid: Grid::new("scheda-pagamenti") }
    }

    fn load(&mut self, cx: &Ctx) {
        let generation = cx.store.generation();
        if generation == self.generation {
            return;
        }
        self.generation = generation;
        self.party = cx.store.party(self.id).ok().flatten();
        let Some(p) = &self.party else { return };
        self.docs = cx.store.documents(&DocFilter { party_id: Some(self.id), ..Default::default() }).unwrap_or_default();
        self.dues = cx.store.due_list_with_drafts(false, None).unwrap_or_default().into_iter().filter(|d| d.party_id == Some(self.id)).collect();
        self.payment = p.default_payment_id.and_then(|id| cx.store.payment_methods().ok()?.into_iter().find(|x| x.id == id)).map(|x| x.name).unwrap_or_default();
        self.list = p.price_list_id.and_then(|id| cx.store.price_lists().ok()?.into_iter().find(|x| x.id == id)).map(|x| x.name).unwrap_or_default();
        self.agent = p.agent_id.and_then(|id| cx.store.agents(true).ok()?.into_iter().find(|x| x.id == id)).map(|x| x.name).unwrap_or_default();
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut Ctx) -> bool {
        self.load(cx);
        let Some(p) = self.party.clone() else {
            cx.msgs.warn("Il soggetto non esiste più");
            return false;
        };
        let mut open = true;
        let mut full = false;
        egui::Window::new(widgets::truncate(&p.display_name(), 70)).id(egui::Id::new("qc_party")).open(&mut open).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.screen_rect().center() + egui::vec2(60.0, 40.0)).default_size([680.0, 540.0]).min_width(480.0).collapsible(false).show(ctx, |ui| {
            egui::TopBottomPanel::bottom("qc_party_bar").show_inside(ui, |ui| {
                ui.add_space(4.0);
                if theme::button(ui, icon::ARROW_SQUARE_OUT, "Apri scheda completa").clicked() {
                    full = true;
                }
            });
            let open_due: Decimal = self.dues.iter().map(|d| if d.receivable { d.residual() } else { -d.residual() }).sum();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.selectable_value(&mut self.tab, 0, "Anagrafica");
                ui.selectable_value(&mut self.tab, 1, format!("Documenti ({})", self.docs.len()));
                ui.selectable_value(&mut self.tab, 2, format!("Pagamenti ({})", self.dues.iter().filter(|d| d.residual() > Decimal::ZERO).count()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !open_due.is_zero() {
                        ui.label(theme::medium(format!("Saldo aperto € {}", eur(open_due))).color(if open_due < Decimal::ZERO { RED } else { theme::INK }));
                    }
                    if !p.active {
                        theme::pill(ui, "Non attivo", Tone::Negative);
                    }
                });
            });
            ui.separator();
            match self.tab {
                0 => {
                    egui::ScrollArea::vertical().id_salt("qc_party_scroll").auto_shrink([false, false]).show(ui, |ui| self.tab_details(ui, &p));
                }
                1 => self.tab_docs(ui, cx),
                _ => self.tab_dues(ui, cx),
            }
        });
        if full {
            cx.nav.push(Nav::FindParty(p.role, p.id, p.display_name()));
            return false;
        }
        open
    }

    fn tab_details(&self, ui: &mut egui::Ui, p: &Party) {
        egui::Grid::new("qc_party_main").num_columns(2).spacing([16.0, 5.0]).show(ui, |ui| {
            kv(ui, "Codice", p.code.clone());
            kv(ui, "Denominazione", p.display_name());
            kv(ui, "Partita IVA", if p.vat_number.is_empty() { String::new() } else { format!("{} {}", p.vat_country, p.vat_number) });
            kv(ui, "Codice fiscale", p.tax_code.clone());
            kv(ui, "Indirizzo", p.full_address());
            kv(ui, "Località", p.city_line());
            kv(ui, "Telefono", p.phone.clone());
            kv(ui, "Cellulare", p.mobile.clone());
            kv(ui, "Email", p.email.clone());
            kv(ui, "PEC", p.pec.clone());
            kv(ui, "Codice SdI", p.sdi_code.clone());
            kv(ui, "Pubblica Amministrazione", if p.is_pa { "Sì" } else { "" });
            kv(ui, "Pagamento", self.payment.clone());
            kv(ui, "Listino", self.list.clone());
            kv(ui, "Sconto", p.default_discount.clone());
            kv(ui, "Agente", self.agent.clone());
            kv(ui, "IBAN", p.iban.clone());
            kv(ui, "Banca", p.bank_name.clone());
            kv(ui, "Mandato SDD", p.sdd_mandate_id.clone());
            kv(ui, "Carta fedeltà", p.loyalty_card.clone());
            kv(ui, "Note", p.notes.clone());
        });
        if !p.contacts.is_empty() {
            ui.add_space(8.0);
            ui.label(theme::medium("Referenti"));
            egui::Grid::new("qc_contacts").num_columns(4).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                for c in &p.contacts {
                    ui.label(&c.name);
                    ui.label(theme::muted(&c.role));
                    ui.label(&c.phone);
                    ui.label(&c.email);
                    ui.end_row();
                }
            });
        }
    }

    fn tab_docs(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let cols: Vec<Col<DocSummary>> = vec![
            Col::new("date", "Data", 90.0, |d: &DocSummary| Val::Date(Some(d.date))),
            Col::text("kind", "Tipo", 170.0, |d: &DocSummary| d.kind.label().to_string()).group(),
            Col::text("num", "N.", 70.0, |d: &DocSummary| d.number_label.clone()),
            Col::text("status", "Stato", 80.0, |d: &DocSummary| d.status.label().to_string()).group(),
            Col::money("total", "Tot. documento", 110.0, |d: &DocSummary| d.total).total(),
            Col::money("open", "Da saldare", 100.0, |d: &DocSummary| d.open_amount).total(),
        ];
        ui.horizontal(|ui| {
            self.doc_grid.toolbar(ui, cx.store, cx.msgs, &cols, &self.docs, "Documenti");
        });
        let h = ui.available_height() - 40.0;
        self.doc_grid.version = 0;
        let ev = self.doc_grid.show(ui, cx.store, &cols, &self.docs, |d| d.id, h, false, 22.0);
        if let Some(id) = ev.opened {
            cx.nav.push(Nav::OpenDocument(id));
        }
    }

    fn tab_dues(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let cols: Vec<Col<DueRow>> = vec![
            Col::new("date", "Scadenza", 90.0, |d: &DueRow| Val::Date(Some(d.date))),
            Col::text("doc", "Documento", 230.0, |d: &DueRow| d.doc_label.clone()),
            Col::text("mp", "Modalità", 140.0, |d: &DueRow| codes::describe(codes::MODALITA_PAGAMENTO, &d.mp_code).to_string()).group(),
            Col::money("amount", "Importo", 100.0, |d: &DueRow| d.amount).total(),
            Col::money("res", "Da saldare", 100.0, |d: &DueRow| d.residual()).total(),
        ];
        ui.horizontal(|ui| {
            self.due_grid.toolbar(ui, cx.store, cx.msgs, &cols, &self.dues, "Pagamenti");
        });
        let h = ui.available_height() - 40.0;
        self.due_grid.version = 0;
        let ev = self.due_grid.show(ui, cx.store, &cols, &self.dues, |d| d.id, h, false, 22.0);
        if let Some(id) = ev.opened.and_then(|k| self.dues.iter().find(|d| d.id == k)).map(|d| d.document_id) {
            cx.nav.push(Nav::OpenDocument(id));
        }
    }
}

struct MoveDialog {
    kind: MoveKind,
    product: Product,
    date: NaiveDate,
    warehouse: String,
    amount: Decimal,
    reason: String,
    origin: String,
    unit_value: Decimal,
    lot: String,
    expiry: Option<NaiveDate>,
    variant: String,
    serial: String,
    reasons: Vec<String>,
    suppliers: Vec<String>,
    focus: bool,
}

const LOAD_REASONS: [&str; 4] = ["Carico", "Carico iniziale", "Reso da cliente", "Produzione interna"];
const UNLOAD_REASONS: [&str; 5] = ["Scarico", "Merce danneggiata", "Omaggio", "Uso interno", "Reso a fornitore"];
const ADJUST_REASONS: [&str; 3] = ["Rettifica giacenza", "Inventario", "Errore di conteggio"];

impl MoveDialog {
    fn new(cx: &Ctx, kind: MoveKind, product: Product) -> Self {
        let defaults: &[&str] = match kind {
            MoveKind::Load => &LOAD_REASONS,
            MoveKind::Unload => &UNLOAD_REASONS,
            MoveKind::Adjust => &ADJUST_REASONS,
        };
        let mut reasons: Vec<String> = defaults.iter().map(|s| s.to_string()).collect();
        for r in cx.store.manual_stock_reasons().unwrap_or_default() {
            if !reasons.contains(&r) {
                reasons.push(r);
            }
        }
        let suppliers = cx.store.parties(&openfatt_store::PartyFilter { role: Some(openfatt_core::model::PartyRole::Supplier), ..Default::default() }).unwrap_or_default().iter().map(|p| p.display_name()).collect();
        let origin = if kind == MoveKind::Load { product.supplier_id.and_then(|id| cx.store.party(id).ok().flatten()).map(|p| p.display_name()).unwrap_or_default() } else { String::new() };
        let warehouse = cx.store.resolve_warehouse("").unwrap_or_default();
        let current = cx.store.stock_in(product.id, &warehouse).unwrap_or_default();
        MoveDialog {
            kind,
            date: chrono::Local::now().date_naive(),
            amount: if kind == MoveKind::Adjust { current } else { Decimal::ZERO },
            reason: defaults[0].to_string(),
            origin,
            unit_value: product.cost,
            warehouse,
            lot: String::new(),
            expiry: None,
            variant: String::new(),
            serial: String::new(),
            reasons,
            suppliers,
            focus: true,
            product,
        }
    }

    fn title(&self) -> (&'static str, &'static str) {
        match self.kind {
            MoveKind::Load => ("Carico magazzino", "Inserimento movimento per aumentare la quantità in giacenza"),
            MoveKind::Unload => ("Scarico magazzino", "Inserimento movimento per diminuire la quantità in giacenza"),
            MoveKind::Adjust => ("Rettifica giacenza magazzino", "Inserimento movimento per aggiustamento della quantità in giacenza"),
        }
    }

    /// Registra il movimento. Restituisce l'esito da mostrare.
    fn commit(&self, cx: &Ctx) -> Result<&'static str, String> {
        let p = &self.product;
        if self.kind == MoveKind::Adjust {
            return match cx.store.adjust_stock(p.id, self.amount, self.date, &self.warehouse, self.reason.trim()).map_err(|e| e.to_string())? {
                Some(_) => Ok("Giacenza rettificata"),
                None => Err("La giacenza è già uguale al valore indicato".into()),
            };
        }
        if self.amount <= Decimal::ZERO {
            return Err("Indicare una quantità maggiore di zero".into());
        }
        let load = self.kind == MoveKind::Load;
        let reason = if self.origin.trim().is_empty() { self.reason.trim().to_string() } else { format!("{} - {}", self.reason.trim(), self.origin.trim()) };
        let m = StockMove { date: self.date, product_id: p.id, qty: if load { self.amount } else { -self.amount }, warehouse: self.warehouse.clone(), unit_value: if load { self.unit_value } else { p.cost }, reason, lot: self.lot.clone(), variant: self.variant.clone(), expiry: self.expiry, serial: self.serial.clone(), ..Default::default() };
        cx.store.add_manual_stock_move(&m).map_err(|e| e.to_string())?;
        Ok(if load { "Carico registrato" } else { "Scarico registrato" })
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut Ctx) -> bool {
        let (title, subtitle) = self.title();
        let mut keep = true;
        let tracked = self.product.tracking != Tracking::None || !self.product.variants.is_empty();
        let warehouses = cx.store.warehouses().unwrap_or_default();
        let current = cx.store.stock_in(self.product.id, &self.warehouse).unwrap_or_default();
        egui::Window::new(title).id(egui::Id::new("qc_move")).collapsible(false).resizable(false).order(egui::Order::Foreground).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(theme::bold(title).size(18.0).color(theme::ACCENT));
            ui.label(theme::muted(subtitle));
            ui.separator();
            if self.kind == MoveKind::Adjust && tracked {
                ui.label(RichText::new("Per gli articoli con varianti, lotti o numeri di serie la giacenza si corregge con Carica e Scarica, indicando variante, lotto o serie.").color(ORANGE));
                ui.add_space(6.0);
                if ui.button("Chiudi").clicked() {
                    keep = false;
                }
                return;
            }
            egui::Grid::new("qc_move_grid").num_columns(2).spacing([12.0, 7.0]).show(ui, |ui| {
                ui.label("Data operazione");
                widgets::date_field(ui, "qc_move_date", &mut self.date);
                ui.end_row();
                if self.kind != MoveKind::Adjust {
                    ui.label(if self.kind == MoveKind::Load { "Provenienza merce" } else { "Destinazione merce" });
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.origin).desired_width(330.0));
                        if self.kind == MoveKind::Load {
                            ui.menu_button(icon::CARET_DOWN, |ui| {
                                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                                    for s in &self.suppliers {
                                        if ui.button(s).clicked() {
                                            self.origin = s.clone();
                                            ui.close_menu();
                                        }
                                    }
                                });
                            });
                        }
                    });
                    ui.end_row();
                }
                ui.label("Prodotto");
                ui.label(theme::medium(format!("{}  {}", self.product.code, widgets::truncate(&self.product.description, 52))));
                ui.end_row();
                if !self.product.supplier_code.is_empty() {
                    ui.label("Cod. prod. fornitore");
                    ui.label(&self.product.supplier_code);
                    ui.end_row();
                }
                if warehouses.len() > 1 {
                    ui.label("Magazzino");
                    widgets::warehouse_combo(ui, "qc_move_wh", &mut self.warehouse, &warehouses, true, 220.0);
                    ui.end_row();
                }
                ui.label(format!("Giac. ({})", if self.product.unit.is_empty() { "pz" } else { &self.product.unit }));
                ui.label(theme::medium(qty(current)));
                ui.end_row();
                ui.label(match self.kind {
                    MoveKind::Load => "Q.tà da caricare",
                    MoveKind::Unload => "Q.tà da scaricare",
                    MoveKind::Adjust => "Nuova giac.",
                });
                ui.horizontal(|ui| {
                    widgets::dec_field(ui, "qc_move_qty", &mut self.amount, 90.0, 4);
                    let after = match self.kind {
                        MoveKind::Load => current + self.amount,
                        MoveKind::Unload => current - self.amount,
                        MoveKind::Adjust => self.amount,
                    };
                    let diff = after - current;
                    ui.label(theme::muted(format!("giacenza dopo: {} ({}{})", qty(after), if diff > Decimal::ZERO { "+" } else { "" }, qty(diff))));
                });
                ui.end_row();
                if self.kind == MoveKind::Load {
                    ui.label("Costo unitario");
                    widgets::dec_field(ui, "qc_move_cost", &mut self.unit_value, 90.0, 4);
                    ui.end_row();
                }
                ui.label("Causale");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.reason).desired_width(330.0));
                    ui.menu_button(icon::CARET_DOWN, |ui| {
                        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                            for r in &self.reasons {
                                if ui.button(r).clicked() {
                                    self.reason = r.clone();
                                    ui.close_menu();
                                }
                            }
                        });
                    });
                });
                ui.end_row();
                if !self.product.variants.is_empty() {
                    ui.label("Variante");
                    egui::ComboBox::from_id_salt("qc_move_var").selected_text(if self.variant.is_empty() { "scegli..." } else { self.variant.as_str() }).show_ui(ui, |ui| {
                        for c in self.product.variants.combinations() {
                            ui.selectable_value(&mut self.variant, c.clone(), c);
                        }
                    });
                    ui.end_row();
                }
                if self.product.tracking == Tracking::Lot {
                    ui.label("Lotto");
                    ui.add(egui::TextEdit::singleline(&mut self.lot).desired_width(160.0).hint_text("in uscita vuoto = automatico"));
                    ui.end_row();
                    ui.label("Scadenza lotto");
                    ui.horizontal(|ui| widgets::opt_date_field(ui, "qc_move_exp", &mut self.expiry));
                    ui.end_row();
                }
                if self.product.tracking == Tracking::Serial {
                    ui.label("Numeri di serie");
                    ui.add(egui::TextEdit::singleline(&mut self.serial).desired_width(330.0).hint_text("separati da virgola"));
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if theme::primary(ui, icon::CHECK, "OK").clicked() {
                    match self.commit(cx) {
                        Ok(m) => {
                            cx.msgs.ok(m);
                            keep = false;
                        }
                        Err(e) => cx.msgs.err(e),
                    }
                }
                if theme::button(ui, "", "Annulla").clicked() {
                    keep = false;
                }
            });
            self.focus = false;
        });
        keep
    }
}

/// Schede aperte: una per i soggetti, una per gli articoli, più l'eventuale movimento.
#[derive(Default)]
pub struct Cards {
    party: Option<PartyCard>,
    product: Option<ProductCard>,
    movement: Option<MoveDialog>,
}

impl Cards {
    /// `allowed` dice se l'utente può consultare anagrafiche e magazzino.
    pub fn show(&mut self, ctx: &egui::Context, cx: &mut Ctx, can_view: bool, can_edit_stock: bool) {
        let pending = ctx.data_mut(|d| {
            let r = d.get_temp::<Request>(request_id());
            d.remove::<Request>(request_id());
            r
        });
        if let Some(req) = pending {
            match req {
                _ if !can_view => cx.msgs.err("L'utente non ha accesso alle anagrafiche"),
                Request::Card(Ref::Party(id)) => self.party = Some(PartyCard::new(id)),
                Request::Card(Ref::Product(id)) => self.product = Some(ProductCard::new(id)),
                Request::Moves(id) => {
                    let mut c = ProductCard::new(id);
                    c.tab = 5;
                    self.product = Some(c);
                }
                Request::Move(..) if !can_edit_stock => cx.msgs.err("L'utente non può registrare movimenti di magazzino"),
                Request::Move(kind, id) => {
                    if let Some(p) = cx.store.product(id).ok().flatten() {
                        self.movement = Some(MoveDialog::new(cx, kind, p));
                    }
                }
            }
        }
        if self.party.as_mut().is_some_and(|c| !c.show(ctx, cx)) {
            self.party = None;
        }
        if self.product.as_mut().is_some_and(|c| !c.show(ctx, cx)) {
            self.product = None;
        }
        if self.movement.as_mut().is_some_and(|m| !m.show(ctx, cx)) {
            self.movement = None;
        }
    }
}

/// Serve per non ricaricare le statistiche di magazzino a ogni disegno della scheda laterale.
#[derive(Default)]
pub struct StockInfoCache {
    data: HashMap<i64, (u64, StockInfo)>,
}

impl StockInfoCache {
    pub fn get(&mut self, cx: &Ctx, p: &Product) -> StockInfo {
        let generation = cx.store.generation();
        match self.data.get(&p.id) {
            Some((g, i)) if *g == generation => i.clone(),
            _ => {
                let i = cx.store.stock_info(p, chrono::Local::now().date_naive()).unwrap_or_default();
                self.data.clear();
                self.data.insert(p.id, (generation, i.clone()));
                i
            }
        }
    }
}
