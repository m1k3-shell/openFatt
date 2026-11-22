//! Cornice della finestra: barra superiore (azienda, ricerca, strumenti), barra dei menu
//! a tendina e barra di stato.

use std::path::PathBuf;
use std::time::Instant;

use eframe::egui::{self, Color32, FontFamily, FontId, RichText, Sense, Stroke, Ui, Vec2};
use openfatt_core::model::{DocKind, DocStatus, PartyRole};
use openfatt_store::{Direction, EInvoiceStatus, Store};

use super::theme::{self, icon, Section, INK, MUTED, NEGATIVE, RAISED};
#[allow(unused_imports)]
use super::theme::INK_SOFT;
use super::business::Extra;
use super::ledger::PayView;
use super::{DocGroup, Nav, Screen};

/// Conteggi mostrati nei menu (ricalcolati ogni pochi secondi).
#[derive(Default)]
pub struct Badges {
    at: Option<Instant>,
    pub rejected: usize,
    pub waiting: usize,
    pub to_send: usize,
    pub received: usize,
    pub draft_quotes: usize,
    pub open_inventory: Option<String>,
    pub recurring_due: usize,
    pub agenda_today: usize,
    generation: u64,
}

impl Badges {
    pub fn invalidate(&mut self) {
        self.at = None;
    }
    pub fn refresh(&mut self, s: &Store) {
        // si ricalcola solo se l'archivio è cambiato (anche da un'altra postazione) o ogni 30 secondi
        let gen = s.generation();
        if self.at.map(|t| t.elapsed().as_secs() < 30).unwrap_or(false) && gen == self.generation {
            return;
        }
        if self.at.map(|t| t.elapsed().as_millis() < 700).unwrap_or(false) {
            return;
        }
        self.generation = gen;
        self.at = Some(Instant::now());
        let today = chrono::Local::now().date_naive();
        self.recurring_due = s.due_recurrences(today).map(|v| v.len()).unwrap_or(0);
        self.agenda_today = s.agenda(today, today).map(|v| v.iter().filter(|a| !a.done).count()).unwrap_or(0);
        let n = |d: Direction, st: EInvoiceStatus| s.einvoice_count(d, st).unwrap_or(0);
        self.rejected = n(Direction::Out, EInvoiceStatus::Rejected);
        self.waiting = n(Direction::Out, EInvoiceStatus::Sent);
        self.to_send = s.documents_without_einvoice().map(|v| v.len()).unwrap_or(0) + n(Direction::Out, EInvoiceStatus::ToSend);
        self.received = n(Direction::In, EInvoiceStatus::Received);
        self.draft_quotes = s
            .documents(&openfatt_store::DocFilter { kinds: vec![DocKind::Quote], status: Some(DocStatus::Draft), ..Default::default() })
            .map(|v| v.len())
            .unwrap_or(0);
        self.open_inventory = s
            .inventory_sessions()
            .unwrap_or_default()
            .into_iter()
            .find(|x| x.status == openfatt_store::InventoryStatus::Open)
            .map(|x| format!("{}: in corso dal {}", x.warehouse, x.date.format("%d/%m/%Y")));
    }
}

#[derive(Default)]
pub struct Search {
    pub text: String,
    pub focus_request: bool,
}

/// Voce di un menu a tendina.
fn menu_item(ui: &mut Ui, ic: &str, title: &str, note: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.min_rect().width().max(270.0), 30.0), Sense::click());
    let p = ui.painter();
    let t = theme::tint();
    if resp.hovered() {
        p.rect_filled(rect, 3, t.soft);
    }
    p.text(egui::pos2(rect.left() + 12.0, rect.center().y), egui::Align2::LEFT_CENTER, ic, FontId::new(17.0, FontFamily::Proportional), t.main);
    p.text(egui::pos2(rect.left() + 40.0, rect.center().y), egui::Align2::LEFT_CENTER, title, FontId::new(13.5, FontFamily::Proportional), INK);
    if !note.is_empty() {
        p.text(egui::pos2(rect.right() - 10.0, rect.center().y), egui::Align2::RIGHT_CENTER, note, FontId::new(12.0, FontFamily::Proportional), MUTED);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn menu_sep(ui: &mut Ui) {
    ui.add_space(3.0);
    let w = ui.min_rect().width().max(270.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), Sense::hover());
    ui.painter().hline(r.x_range().shrink(4.0), r.center().y, Stroke::new(1.0_f32, theme::RULE));
    ui.add_space(3.0);
}

/// Pulsante della barra: icona grande sopra, nome sotto.
fn tool_button(ui: &mut Ui, sec: Section, ic: &str, text: &str, current: bool, badge: usize, dropdown: bool) -> egui::Response {
    let font = FontId::new(12.0, theme::medium_family());
    let label = if dropdown { format!("{} {}", text, icon::CARET_DOWN) } else { text.to_string() };
    let tw = ui.fonts(|f| f.layout_no_wrap(label.clone(), font.clone(), INK).size().x);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new((tw + 14.0).max(52.0), 56.0), Sense::click());
    let p = ui.painter();
    let t = sec.tint();
    // ogni pulsante ha il colore della sua sezione; quello corrente è pieno e sottolineato
    if current {
        p.rect_filled(rect, 4, t.soft);
        p.rect_filled(egui::Rect::from_min_max(egui::pos2(rect.left() + 6.0, rect.bottom() - 3.0), egui::pos2(rect.right() - 6.0, rect.bottom())), 2, t.main);
    } else if resp.hovered() {
        p.rect_filled(rect, 4, t.tint);
    }
    p.text(egui::pos2(rect.center().x, rect.top() + 21.0), egui::Align2::CENTER_CENTER, ic, FontId::new(25.0, FontFamily::Proportional), t.main);
    p.text(egui::pos2(rect.center().x, rect.bottom() - 11.0), egui::Align2::CENTER_CENTER, label, font, if current { t.dark } else { INK });
    if badge > 0 {
        let c = egui::pos2(rect.center().x + 15.0, rect.top() + 10.0);
        let t = badge.to_string();
        let w = (t.len() as f32 * 6.0 + 8.0).max(15.0);
        p.rect_filled(egui::Rect::from_center_size(c, Vec2::new(w, 15.0)), 8, NEGATIVE);
        p.text(c, egui::Align2::CENTER_CENTER, t, FontId::new(10.0, theme::medium_family()), Color32::WHITE);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Pulsante della barra che apre un menu a tendina.
fn tool_menu(ui: &mut Ui, sec: Section, ic: &str, text: &str, current: bool, badge: usize, add: impl FnOnce(&mut Ui)) {
    let resp = tool_button(ui, sec, ic, text, current, badge, true);
    let id = ui.make_persistent_id(("tool_menu", text));
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(id));
    }
    egui::popup_below_widget(ui, id, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
        ui.set_min_width(270.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        theme::with_section(sec, || add(ui));
    });
}

fn tool_sep(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(9.0, 44.0), Sense::hover());
    ui.painter().vline(r.center().x, r.y_range(), Stroke::new(1.0_f32, theme::BORDER));
}

/// Sezione (e quindi colore) di una schermata; per il documento aperto conta il tipo.
pub fn section_of(s: Screen, editing: Option<DocKind>) -> Section {
    match s {
        Screen::Home => Section::Start,
        Screen::Parties(PartyRole::Supplier) => Section::Suppliers,
        Screen::Parties(_) => Section::Customers,
        Screen::Products | Screen::PriceLists => Section::Products,
        Screen::Counter => Section::Counter,
        Screen::Documents(DocGroup::Production) => Section::Stock,
        Screen::Documents(DocGroup::SelfInvoices) => Section::EInvoice,
        Screen::Documents(g) if DocGroup::PURCHASES.contains(&g) => Section::Purchases,
        Screen::Documents(_) => Section::Sales,
        Screen::EInvoicesOut | Screen::EInvoicesIn => Section::EInvoice,
        Screen::Stock | Screen::Inventory => Section::Stock,
        Screen::Dues | Screen::Bank => Section::Payments,
        Screen::Cash | Screen::Vat => Section::Tools,
        Screen::Editor => match editing {
            Some(k) if k.is_purchase() => Section::Purchases,
            Some(DocKind::RetailSale) => Section::Counter,
            Some(DocKind::SelfInvoice) => Section::EInvoice,
            Some(k) if k.is_internal() => Section::Stock,
            _ => Section::Sales,
        },
        Screen::Settings => Section::Options,
        Screen::Tools | Screen::Backup => Section::Archives,
        Screen::Extra(x) => match x {
            Extra::Agenda => Section::Agenda,
            Extra::Agents | Extra::Stats => Section::Analysis,
            Extra::Recurring => Section::Sales,
            Extra::Conservation => Section::EInvoice,
            _ => Section::Tools,
        },
    }
}

/// Unica barra in alto: le sezioni del programma come pulsanti con icona, sempre visibili.
#[allow(clippy::too_many_arguments)]
pub fn tool_bar(ui: &mut Ui, store: &Store, recent: &[PathBuf], search: &mut Search, pec_busy: bool, screen: Screen, editor: Option<(String, bool, DocKind)>, b: &Badges, nav: &mut Vec<Nav>) {
    // il documento aperto non accende nessun pulsante della barra
    let cur = if screen == Screen::Editor { None } else { Some(section_of(screen, None)) };
    let is = |s: Section| cur == Some(s);
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        tool_menu(ui, Section::Sales, icon::PLUS_CIRCLE, "Nuovo", false, 0, |ui| {
            for (ic, k) in [
                (icon::NOTE_PENCIL, DocKind::Quote),
                (icon::CLIPBOARD_TEXT, DocKind::CustomerOrder),
                (icon::TRUCK_TRAILER, DocKind::DeliveryNote),
                (icon::FILE_TEXT, DocKind::Invoice),
                (icon::FILE_TEXT, DocKind::AccompanyingInvoice),
                (icon::FILE_MINUS, DocKind::CreditNote),
                (icon::FILE_DASHED, DocKind::Proforma),
            ] {
                if menu_item(ui, ic, k.label(), "") {
                    nav.push(Nav::NewDocument(k));
                }
            }
            menu_sep(ui);
            theme::with_section(Section::Purchases, || {
                for (ic, k) in [(icon::NOTE_PENCIL, DocKind::SupplierQuote), (icon::CLIPBOARD_TEXT, DocKind::SupplierOrder), (icon::PACKAGE, DocKind::GoodsReceipt), (icon::FILE_TEXT, DocKind::PurchaseInvoice)] {
                    if menu_item(ui, ic, k.label(), "") {
                        nav.push(Nav::NewDocument(k));
                    }
                }
            });
            menu_sep(ui);
            if menu_item(ui, icon::ARROWS_COUNTER_CLOCKWISE, DocKind::SelfInvoice.label(), "") {
                nav.push(Nav::Go(Screen::Documents(DocGroup::SelfInvoices)));
            }
        });
        if tool_button(ui, Section::Counter, icon::CASH_REGISTER, "Banco", is(Section::Counter), 0, false).on_hover_text("Vendita al banco (F9)").clicked() {
            nav.push(Nav::Go(Screen::Counter));
        }
        tool_sep(ui);
        if tool_button(ui, Section::Start, icon::HOUSE, "Start", is(Section::Start), 0, false).clicked() {
            nav.push(Nav::Go(Screen::Home));
        }
        if tool_button(ui, Section::Customers, icon::USERS, "Clienti", is(Section::Customers), 0, false).clicked() {
            nav.push(Nav::Go(Screen::Parties(PartyRole::Customer)));
        }
        if tool_button(ui, Section::Suppliers, icon::TRUCK, "Fornitori", is(Section::Suppliers), 0, false).clicked() {
            nav.push(Nav::Go(Screen::Parties(PartyRole::Supplier)));
        }
        if tool_button(ui, Section::Products, icon::PACKAGE, "Prodotti", is(Section::Products), 0, false).clicked() {
            nav.push(Nav::Go(Screen::Products));
        }
        tool_menu(ui, Section::Sales, icon::FILES, "Documenti", is(Section::Sales) || is(Section::Purchases), 0, |ui| {
            for g in DocGroup::SALES.iter().filter(|g| **g != DocGroup::SelfInvoices) {
                if menu_item(ui, icon::FILE_TEXT, g.label(), "") {
                    nav.push(Nav::Go(Screen::Documents(*g)));
                }
            }
            menu_sep(ui);
            theme::with_section(Section::Purchases, || {
                for g in DocGroup::PURCHASES {
                    if menu_item(ui, icon::SHOPPING_CART, g.label(), "") {
                        nav.push(Nav::Go(Screen::Documents(g)));
                    }
                }
            });
            menu_sep(ui);
            let n = if b.recurring_due > 0 { b.recurring_due.to_string() } else { String::new() };
            if menu_item(ui, icon::REPEAT, "Documenti ricorrenti", &n) {
                nav.push(Nav::Go(Screen::Extra(Extra::Recurring)));
            }
        });
        tool_menu(ui, Section::EInvoice, icon::PAPER_PLANE_TILT, "Fatture elettr.", is(Section::EInvoice), b.rejected, |ui| {
            if menu_item(ui, icon::PAPER_PLANE_TILT, "Fatture inviate", &if b.to_send > 0 { format!("{} da inviare", b.to_send) } else { String::new() }) {
                nav.push(Nav::Go(Screen::EInvoicesOut));
            }
            if menu_item(ui, icon::TRAY_ARROW_DOWN, "Fatture ricevute", &if b.received > 0 { format!("{} da registrare", b.received) } else { String::new() }) {
                nav.push(Nav::Go(Screen::EInvoicesIn));
            }
            if menu_item(ui, icon::ARROWS_COUNTER_CLOCKWISE, "Autofatture e integrazioni", "") {
                nav.push(Nav::Go(Screen::Documents(DocGroup::SelfInvoices)));
            }
            menu_sep(ui);
            if menu_item(ui, icon::ENVELOPE_SIMPLE, if pec_busy { "Ricezione in corso..." } else { "Ricevi dalla PEC" }, "F5") {
                nav.push(Nav::PecFetch);
            }
            if menu_item(ui, icon::ARCHIVE, "Conservazione e ordini NSO", "") {
                nav.push(Nav::Go(Screen::Extra(Extra::Conservation)));
            }
        });
        tool_menu(ui, Section::Payments, icon::CURRENCY_CIRCLE_DOLLAR, "Pagamenti", is(Section::Payments), 0, |ui| {
            for (ic, t, v) in [(icon::ARROWS_DOWN_UP, "Tutti", PayView::All), (icon::ARROW_UP, "Entrate", PayView::In), (icon::ARROW_DOWN, "Uscite", PayView::Out)] {
                if menu_item(ui, ic, t, "") {
                    nav.push(Nav::Payments(v));
                }
            }
            menu_sep(ui);
            if menu_item(ui, icon::CALENDAR_CHECK, "Da saldare al...", "") {
                nav.push(Nav::Payments(PayView::Until));
            }
            if menu_item(ui, icon::ENVELOPE_SIMPLE, "Invio solleciti", "") {
                nav.push(Nav::Reminders);
            }
            if menu_item(ui, icon::BANK, "RIBA da emettere", "") {
                nav.push(Nav::BankTab(super::bank::Tab::Riba));
            }
            if menu_item(ui, icon::BANK, "Addebiti SDD da emettere", "") {
                nav.push(Nav::BankTab(super::bank::Tab::Sdd));
            }
            if menu_item(ui, icon::BANK, "Bonifici da pagare", "") {
                nav.push(Nav::BankTab(super::bank::Tab::Sct));
            }
            if menu_item(ui, icon::LIST_CHECKS, "Distinte emesse", "") {
                nav.push(Nav::BankTab(super::bank::Tab::Batches));
            }
            menu_sep(ui);
            if menu_item(ui, icon::GEAR_SIX, "Impostazioni banche e risorse...", "") {
                nav.push(Nav::BankSettings);
            }
        });
        tool_menu(ui, Section::Stock, icon::WAREHOUSE, "Magazzino", is(Section::Stock), 0, |ui| {
            if menu_item(ui, icon::STACK, "Giacenze e movimenti", "") {
                nav.push(Nav::Go(Screen::Stock));
            }
            if menu_item(ui, icon::LIST_CHECKS, "Inventario", "") {
                nav.push(Nav::Go(Screen::Inventory));
            }
            if menu_item(ui, icon::FACTORY, "Produzione", "") {
                nav.push(Nav::Go(Screen::Documents(DocGroup::Production)));
            }
        });
        if tool_button(ui, Section::Agenda, icon::CALENDAR_BLANK, "Agenda", is(Section::Agenda), b.agenda_today, false).clicked() {
            nav.push(Nav::Go(Screen::Extra(Extra::Agenda)));
        }
        tool_menu(ui, Section::Analysis, icon::CHART_LINE_UP, "Analisi", is(Section::Analysis), 0, |ui| {
            if menu_item(ui, icon::CHART_LINE_UP, "Statistiche e flussi", "") {
                nav.push(Nav::Go(Screen::Extra(Extra::Stats)));
            }
            if menu_item(ui, icon::HANDSHAKE, "Agenti e provvigioni", "") {
                nav.push(Nav::Go(Screen::Extra(Extra::Agents)));
            }
        });
        tool_sep(ui);
        tool_menu(ui, Section::Tools, icon::WRENCH, "Strumenti", is(Section::Tools), 0, |ui| {
            if menu_item(ui, icon::PERCENT, "Registri e liquidazione IVA", "") {
                nav.push(Nav::Go(Screen::Vat));
            }
            if menu_item(ui, icon::WALLET, "Prima nota", "") {
                nav.push(Nav::Go(Screen::Cash));
            }
            if menu_item(ui, icon::CHART_BAR, "Corrispettivi", "") {
                nav.push(Nav::CounterTab("corrispettivi"));
            }
            if menu_item(ui, icon::TAG, "Listini prezzi", "") {
                nav.push(Nav::Go(Screen::PriceLists));
            }
            menu_sep(ui);
            if menu_item(ui, icon::GIFT, "Carte fedeltà e buoni", "") {
                nav.push(Nav::Go(Screen::Extra(Extra::Cards)));
            }
            if menu_item(ui, icon::STOREFRONT, "Negozio online", "") {
                nav.push(Nav::Go(Screen::Extra(Extra::Shop)));
            }
            if menu_item(ui, icon::CALENDAR_X, "Operazioni di fine anno", "") {
                nav.push(Nav::Go(Screen::Extra(Extra::YearEnd)));
            }
        });
        if tool_button(ui, Section::Options, icon::GEAR_SIX, "Opzioni", is(Section::Options), 0, false).clicked() {
            nav.push(Nav::Go(Screen::Settings));
        }
        tool_menu(ui, Section::Archives, icon::FOLDER_OPEN, "Archivi", is(Section::Archives), 0, |ui| {
            if menu_item(ui, icon::FOLDER_OPEN, "Apri un altro archivio...", "") {
                if let Some(p) = rfd::FileDialog::new().add_filter("Archivio openFatt", &["gadb"]).pick_file() {
                    nav.push(Nav::OpenArchive(p));
                }
            }
            let others: Vec<&PathBuf> = recent.iter().filter(|r| r.as_path() != store.path() && r.exists()).take(4).collect();
            for r in others {
                let n = r.file_stem().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                if menu_item(ui, icon::DATABASE, &n, "") {
                    nav.push(Nav::OpenArchive(r.clone()));
                }
            }
            menu_sep(ui);
            if menu_item(ui, icon::SHIELD_CHECK, "Copie di sicurezza", "") {
                nav.push(Nav::Go(Screen::Backup));
            }
            if menu_item(ui, icon::DATABASE, "Importazioni ed esportazioni", "") {
                nav.push(Nav::Go(Screen::Tools));
            }
            if menu_item(ui, icon::SIGN_OUT, "Chiudi archivio", "") {
                nav.push(Nav::CloseArchive);
            }
        });
        if let Some((label, dirty, kind)) = editor.filter(|_| screen != Screen::Editor) {
            tool_sep(ui);
            let t = if dirty { "Doc. aperto *" } else { "Doc. aperto" };
            if tool_button(ui, section_of(Screen::Editor, Some(kind)), icon::PENCIL_SIMPLE, t, false, 0, false).on_hover_text(label).clicked() {
                nav.push(Nav::Go(Screen::Editor));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            global_search(ui, store, search, 230.0, nav);
        });
    });
}

fn global_search(ui: &mut Ui, store: &Store, st: &mut Search, width: f32, nav: &mut Vec<Nav>) {
    let id = egui::Id::new("global_search");
    let frame = egui::Frame::new().fill(theme::SURFACE).stroke(Stroke::new(1.0_f32, theme::CONTROL)).corner_radius(6).inner_margin(egui::Margin::symmetric(10, 0));
    let resp = frame
        .show(ui, |ui| {
            ui.set_width(width - 20.0);
            ui.set_height(26.0);
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new(icon::MAGNIFYING_GLASS).size(16.0).color(MUTED));
                let te = egui::TextEdit::singleline(&mut st.text).id(id).frame(false).hint_text("Cerca ovunque").desired_width(width - 86.0);
                let r = ui.add(te);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("Ctrl K").size(12.0).color(MUTED));
                });
                r
            })
            .inner
        })
        .inner;
    if st.focus_request {
        resp.request_focus();
        st.focus_request = false;
    }
    if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        st.text.clear();
        resp.surrender_focus();
    }
    let live = st.text.trim().to_string();
    if live.len() < 2 {
        return;
    }
    // si cerca quando chi scrive si ferma, e solo i primi risultati di ogni gruppo
    let q = super::settled(ui.ctx(), "global_search", &live);
    if q.len() < 2 {
        return;
    }
    let (found, _) = super::memo(ui.ctx(), "global_search", store.generation(), q.clone(), || {
        let parties: Vec<_> = store.parties(&openfatt_store::PartyFilter { role: None, search: q.clone(), include_inactive: false }).unwrap_or_default().into_iter().take(5).collect();
        let products = store.products_page(&q, false, 5).unwrap_or_default();
        let docs = store.documents(&openfatt_store::DocFilter { search: q.clone(), limit: Some(6), ..Default::default() }).unwrap_or_default();
        (parties, products, docs)
    });
    let (parties, products, docs) = (&found.0, &found.1, &found.2);
    let pos = resp.rect.left_bottom() + Vec2::new(-30.0, 10.0);
    let mut chosen = false;
    egui::Area::new(id.with("pop")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_width(width + 40.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut any = false;
            let head = |ui: &mut Ui, t: &str| {
                ui.add_space(6.0);
                ui.label(RichText::new(format!("  {}", t)).size(12.0).color(MUTED));
                ui.add_space(2.0);
            };
            if !parties.is_empty() {
                head(ui, "Clienti e fornitori");
                for p in parties.iter().take(5) {
                    any = true;
                    let ic = if p.role == PartyRole::Supplier { icon::TRUCK } else { icon::USERS };
                    let desc = format!("{} {}", p.city, if p.vat_number.is_empty() { &p.tax_code } else { &p.vat_number });
                    if result_row(ui, ic, &p.display_name(), &desc, width + 28.0) {
                        nav.push(Nav::FindParty(p.role, p.id, p.display_name()));
                        chosen = true;
                    }
                }
            }
            if !products.is_empty() {
                head(ui, "Articoli");
                for p in products.iter().take(5) {
                    any = true;
                    if result_row(ui, icon::PACKAGE, &p.description, &format!("{}  ·  € {}", p.code, super::widgets::eur(p.price)), width + 28.0) {
                        nav.push(Nav::FindProduct(p.id, p.code.clone()));
                        chosen = true;
                    }
                }
            }
            if !docs.is_empty() {
                head(ui, "Documenti");
                for d in docs.iter().take(6) {
                    any = true;
                    let t = format!("{} {}", d.kind.label(), d.number_label);
                    let desc = format!("{}  ·  {}  ·  € {}", d.date.format("%d/%m/%Y"), d.party_name, super::widgets::eur(d.total));
                    if result_row(ui, icon::FILE_TEXT, &t, &desc, width + 28.0) {
                        nav.push(Nav::OpenDocument(d.id));
                        chosen = true;
                    }
                }
            }
            if !any {
                ui.add_space(8.0);
                ui.label(RichText::new(format!("  Nessun risultato per \"{}\"", q)).color(MUTED));
                ui.add_space(8.0);
            }
        });
    });
    if chosen {
        st.text.clear();
    }
}

fn result_row(ui: &mut Ui, ic: &str, title: &str, desc: &str, w: f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 44.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 6, RAISED);
    }
    p.text(egui::pos2(rect.left() + 10.0, rect.center().y), egui::Align2::LEFT_CENTER, ic, FontId::new(18.0, FontFamily::Proportional), INK_SOFT);
    p.text(egui::pos2(rect.left() + 38.0, rect.top() + 6.0), egui::Align2::LEFT_TOP, title, FontId::new(14.0, theme::medium_family()), INK);
    p.text(egui::pos2(rect.left() + 38.0, rect.top() + 25.0), egui::Align2::LEFT_TOP, desc, FontId::new(12.0, FontFamily::Proportional), MUTED);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Barra di stato in fondo alla finestra.
pub fn status_bar(ui: &mut Ui, store: Option<&Store>, pec_busy: Option<&str>, pec_ok: bool, backup: Option<(crate::backups::Health, String)>, nav: &mut Vec<Nav>) {
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        let small = |t: &str| RichText::new(t).size(12.0).color(MUTED);
        if let Some(s) = store {
            let name = s.company().map(|c| c.display_name()).unwrap_or_default();
            ui.label(RichText::new(if name.is_empty() { "Azienda da configurare".to_string() } else { name }).size(12.0).color(INK));
            ui.label(small(&s.path().display().to_string()));
        } else {
            ui.label(small("Nessun archivio aperto"));
        }
        if let Some(b) = pec_busy {
            ui.spinner();
            ui.label(small(b));
        } else if store.is_some() {
            let (dot, t) = if pec_ok { (theme::POSITIVE, "PEC configurata") } else { (MUTED, "PEC da configurare in Opzioni") };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, dot);
                ui.label(small(t));
            });
        }
        if let Some((h, t)) = backup {
            use crate::backups::Health;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if h == Health::Running {
                    ui.add(egui::Spinner::new().size(12.0));
                } else {
                    let c = match h {
                        Health::Good => theme::POSITIVE,
                        Health::Warn => Color32::from_rgb(0xd9, 0x8a, 0x00),
                        _ => NEGATIVE,
                    };
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, c);
                }
                let color = if h == Health::Bad { NEGATIVE } else { MUTED };
                if ui.add(egui::Label::new(RichText::new(t).size(12.0).color(color)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    nav.push(Nav::Go(Screen::Backup));
                }
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(small(&format!("openFatt {}", env!("CARGO_PKG_VERSION"))));
        });
    });
}
