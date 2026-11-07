//! Cornice della finestra: barra superiore (azienda, ricerca, strumenti), barra dei menu
//! a tendina e barra di stato.

use std::path::PathBuf;
use std::time::Instant;

use eframe::egui::{self, Color32, FontFamily, FontId, RichText, Sense, Stroke, Ui, Vec2};
use openfatt_core::model::{DocKind, DocStatus, PartyRole};
use openfatt_store::{Direction, EInvoiceStatus, Store};

use super::theme::{self, icon, ACCENT, ACTIVE, INK, INK_SOFT, MUTED, NEGATIVE, RAISED};
use super::business::Extra;
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

/// Voce di un menu a tendina: icona, titolo, descrizione e scorciatoia.
fn menu_item(ui: &mut Ui, ic: &str, title: &str, desc: &str, shortcut: &str) -> bool {
    let h = if desc.is_empty() { 36.0 } else { 50.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(360.0, h), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 6, RAISED);
    }
    let top = if desc.is_empty() { rect.center().y - 9.0 } else { rect.top() + 9.0 };
    p.text(egui::pos2(rect.left() + 12.0, top), egui::Align2::LEFT_TOP, ic, FontId::new(18.0, FontFamily::Proportional), Color32::from_rgb(0x4a, 0x4f, 0x57));
    p.text(egui::pos2(rect.left() + 42.0, top), egui::Align2::LEFT_TOP, title, FontId::new(14.0, theme::medium_family()), INK);
    if !desc.is_empty() {
        p.text(egui::pos2(rect.left() + 42.0, top + 19.0), egui::Align2::LEFT_TOP, desc, FontId::new(12.0, FontFamily::Proportional), MUTED);
    }
    if !shortcut.is_empty() {
        p.text(egui::pos2(rect.right() - 12.0, top + 1.0), egui::Align2::RIGHT_TOP, shortcut, FontId::new(12.0, FontFamily::Proportional), MUTED);
    }
    if resp.clicked() {
        ui.close_menu();
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn menu_sep(ui: &mut Ui) {
    ui.add_space(2.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(360.0, 1.0), Sense::hover());
    ui.painter().hline(r.x_range().shrink(4.0), r.center().y, Stroke::new(1.0_f32, theme::RULE));
    ui.add_space(2.0);
}

/// Pulsante di primo livello della barra dei menu.
fn top_button(ui: &mut Ui, ic: &str, text: &str, current: bool, badge: usize, dropdown: bool, add: impl FnOnce(&mut Ui)) -> egui::Response {
    let mut job = egui::text::LayoutJob::default();
    let fmt = |size: f32, fam: FontFamily, c: Color32| egui::TextFormat { font_id: FontId::new(size, fam), color: c, valign: egui::Align::Center, ..Default::default() };
    job.append(ic, 0.0, fmt(18.0, FontFamily::Proportional, INK_SOFT));
    job.append(text, 8.0, fmt(14.0, theme::medium_family(), if current { INK } else { INK_SOFT }));
    if badge > 0 {
        // spazio per il numero disegnato sopra
        job.append("", 28.0, fmt(14.0, FontFamily::Proportional, INK));
    }
    if dropdown {
        job.append(icon::CARET_DOWN, 6.0, fmt(12.0, FontFamily::Proportional, MUTED));
    }
    let resp = ui
        .scope(|ui| {
            theme::flat_visuals(ui, if current { ACTIVE } else { Color32::TRANSPARENT }, if current { ACTIVE } else { RAISED }, ACTIVE);
            ui.spacing_mut().button_padding = Vec2::new(10.0, 6.0);
            let b = egui::Button::new(job).min_size(Vec2::new(0.0, 36.0));
            if dropdown {
                egui::menu::menu_custom_button(ui, b, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    add(ui);
                })
                .response
            } else {
                ui.add(b)
            }
        })
        .inner;
    if badge > 0 {
        let c = egui::pos2(resp.rect.right() - if dropdown { 40.0 } else { 22.0 }, resp.rect.center().y);
        let t = badge.to_string();
        let w = (t.len() as f32 * 7.0 + 10.0).max(18.0);
        ui.painter().rect_filled(egui::Rect::from_center_size(c, Vec2::new(w, 18.0)), 9, NEGATIVE);
        ui.painter().text(c, egui::Align2::CENTER_CENTER, t, FontId::new(11.0, theme::medium_family()), Color32::WHITE);
    }
    resp
}

fn group_of(s: Screen) -> &'static str {
    match s {
        Screen::Home => "home",
        Screen::Parties(_) | Screen::Products | Screen::PriceLists => "anagrafiche",
        Screen::Counter => "vendite",
        Screen::Documents(DocGroup::Production) => "magazzino",
        Screen::Documents(g) if DocGroup::SALES.contains(&g) => "vendite",
        Screen::Documents(_) => "acquisti",
        Screen::EInvoicesOut | Screen::EInvoicesIn => "fe",
        Screen::Stock | Screen::Inventory => "magazzino",
        Screen::Dues | Screen::Cash | Screen::Bank | Screen::Vat => "contabilita",
        Screen::Editor => "editor",
        Screen::Settings | Screen::Tools => "sistema",
        Screen::Extra(x) => x.group(),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {}", one)
    } else {
        format!("{} {}", n, many)
    }
}

/// Barra dei menu a tendina.
pub fn menu_bar(ui: &mut Ui, screen: Screen, editor: Option<(String, bool)>, b: &Badges, nav: &mut Vec<Nav>) {
    let cur = group_of(screen);
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if top_button(ui, icon::HOUSE, "Scrivania", cur == "home", 0, false, |_| {}).clicked() {
            nav.push(Nav::Go(Screen::Home));
        }
        top_button(ui, icon::ADDRESS_BOOK, "Anagrafiche", cur == "anagrafiche", 0, true, |ui| {
            if menu_item(ui, icon::USERS, "Clienti", "Anagrafiche, condizioni e listino", "") {
                nav.push(Nav::Go(Screen::Parties(PartyRole::Customer)));
            }
            if menu_item(ui, icon::TRUCK, "Fornitori", "Anagrafiche e documenti di acquisto", "") {
                nav.push(Nav::Go(Screen::Parties(PartyRole::Supplier)));
            }
            menu_sep(ui);
            if menu_item(ui, icon::PACKAGE, "Articoli", "Prezzi, varianti, lotti e codici a barre", "") {
                nav.push(Nav::Go(Screen::Products));
            }
            if menu_item(ui, icon::TAG, "Listini prezzi", "Listini e condizioni per cliente", "") {
                nav.push(Nav::Go(Screen::PriceLists));
            }
        });
        top_button(ui, icon::RECEIPT, "Vendite", cur == "vendite", 0, true, |ui| {
            if menu_item(ui, icon::CASH_REGISTER, "Vendita al banco", "Scontrino tramite registratore telematico", "F9") {
                nav.push(Nav::Go(Screen::Counter));
            }
            menu_sep(ui);
            let quotes = if b.draft_quotes > 0 { plural(b.draft_quotes, "in bozza", "in bozza") } else { "Offerte ai clienti".into() };
            let items: [(&str, DocGroup, String); 6] = [
                (icon::NOTE_PENCIL, DocGroup::Quotes, quotes),
                (icon::CLIPBOARD_TEXT, DocGroup::Orders, "Impegnano la merce a magazzino".into()),
                (icon::TRUCK_TRAILER, DocGroup::DeliveryNotes, "Documenti di trasporto".into()),
                (icon::FILE_TEXT, DocGroup::Invoices, "Fatture, note di credito e di debito, parcelle".into()),
                (icon::FILE_DASHED, DocGroup::Proforma, "Senza valore fiscale".into()),
                (icon::ARROWS_COUNTER_CLOCKWISE, DocGroup::SelfInvoices, "TD16-TD29: reverse charge e integrazioni".into()),
            ];
            for (ic, g, d) in items {
                if menu_item(ui, ic, g.label(), &d, "") {
                    nav.push(Nav::Go(Screen::Documents(g)));
                }
            }
            menu_sep(ui);
            let rec = if b.recurring_due > 0 { plural(b.recurring_due, "da generare", "da generare") } else { "Canoni, abbonamenti, affitti".into() };
            if menu_item(ui, icon::REPEAT, "Documenti ricorrenti", &rec, "") {
                nav.push(Nav::Go(Screen::Extra(Extra::Recurring)));
            }
            if menu_item(ui, icon::GIFT, "Carte fedeltà e buoni", "Raccolta punti e gift card", "") {
                nav.push(Nav::Go(Screen::Extra(Extra::Cards)));
            }
            if menu_item(ui, icon::STOREFRONT, "Negozio online", "Ordini e articoli WooCommerce o Shopify", "") {
                nav.push(Nav::Go(Screen::Extra(Extra::Shop)));
            }
        });
        top_button(ui, icon::SHOPPING_CART, "Acquisti", cur == "acquisti", 0, true, |ui| {
            let items: [(&str, DocGroup, &str); 3] = [
                (icon::CLIPBOARD_TEXT, DocGroup::SupplierOrders, "Merce ordinata in arrivo"),
                (icon::PACKAGE, DocGroup::GoodsReceipts, "Carico a magazzino"),
                (icon::FILE_TEXT, DocGroup::PurchaseInvoices, "Registrazione e scadenze dei pagamenti"),
            ];
            for (ic, g, d) in items {
                if menu_item(ui, ic, g.label(), d, "") {
                    nav.push(Nav::Go(Screen::Documents(g)));
                }
            }
        });
        top_button(ui, icon::PAPER_PLANE_TILT, "Fattura elettronica", cur == "fe", b.rejected, true, |ui| {
            let mut out = Vec::new();
            if b.rejected > 0 {
                out.push(plural(b.rejected, "scartata", "scartate"));
            }
            if b.waiting > 0 {
                out.push(format!("{} in attesa di esito", b.waiting));
            }
            if b.to_send > 0 {
                out.push(format!("{} da inviare", b.to_send));
            }
            let out = if out.is_empty() { "Stato delle fatture trasmesse".to_string() } else { out.join(", ") };
            if menu_item(ui, icon::PAPER_PLANE_TILT, "Inviate allo SdI", &out, "") {
                nav.push(Nav::Go(Screen::EInvoicesOut));
            }
            let inc = if b.received > 0 { format!("{} da registrare", b.received) } else { "Fatture dei fornitori".into() };
            if menu_item(ui, icon::TRAY_ARROW_DOWN, "Ricevute dai fornitori", &inc, "") {
                nav.push(Nav::Go(Screen::EInvoicesIn));
            }
            menu_sep(ui);
            if menu_item(ui, icon::ENVELOPE_SIMPLE, "Ricevi dalla PEC", "Fatture e notifiche dello SdI", "F5") {
                nav.push(Nav::PecFetch);
            }
            if menu_item(ui, icon::ARCHIVE, "Conservazione e ordini NSO", "Pacchetto per il conservatore, ordini della PA", "") {
                nav.push(Nav::Go(Screen::Extra(Extra::Conservation)));
            }
        });
        top_button(ui, icon::WAREHOUSE, "Magazzino", cur == "magazzino", 0, true, |ui| {
            if menu_item(ui, icon::STACK, "Giacenze e movimenti", "Per magazzino, variante e lotto; trasferimenti", "") {
                nav.push(Nav::Go(Screen::Stock));
            }
            let inv = b.open_inventory.clone().unwrap_or_else(|| "Conta e rettifica delle giacenze".into());
            if menu_item(ui, icon::LIST_CHECKS, "Inventario", &inv, "") {
                nav.push(Nav::Go(Screen::Inventory));
            }
            if menu_item(ui, icon::FACTORY, "Produzione", "Distinte base: carico dei prodotti finiti", "") {
                nav.push(Nav::Go(Screen::Documents(DocGroup::Production)));
            }
        });
        top_button(ui, icon::BANK, "Contabilità", cur == "contabilita", 0, true, |ui| {
            if menu_item(ui, icon::CALENDAR_CHECK, "Scadenzario", "Incassi e pagamenti", "") {
                nav.push(Nav::Go(Screen::Dues));
            }
            if menu_item(ui, icon::PERCENT, "Registri e liquidazione IVA", "Registri vendite e acquisti, liquidazione, Comunicazione LIPE", "") {
                nav.push(Nav::Go(Screen::Vat));
            }
            if menu_item(ui, icon::BANK, "Distinte bancarie", "Ri.Ba., addebiti SDD e bonifici per l'home banking", "") {
                nav.push(Nav::Go(Screen::Bank));
            }
            if menu_item(ui, icon::WALLET, "Prima nota", "Cassa e banca", "") {
                nav.push(Nav::Go(Screen::Cash));
            }
            if menu_item(ui, icon::CHART_BAR, "Corrispettivi", "Riepilogo giornaliero del banco", "") {
                nav.push(Nav::CounterTab("corrispettivi"));
            }
            menu_sep(ui);
            if menu_item(ui, icon::CALENDAR_X, "Operazioni di fine anno", "Controlli, inventario al 31/12, chiusura", "") {
                nav.push(Nav::Go(Screen::Extra(Extra::YearEnd)));
            }
        });
        top_button(ui, icon::CHART_LINE_UP, "Analisi", cur == "analisi", 0, true, |ui| {
            if menu_item(ui, icon::CHART_LINE_UP, "Statistiche e flussi", "Venduto, margini, confronto anni, cassa prevista", "") {
                nav.push(Nav::Go(Screen::Extra(Extra::Stats)));
            }
            if menu_item(ui, icon::HANDSHAKE, "Agenti e provvigioni", "Maturato, insoluti, liquidazioni", "") {
                nav.push(Nav::Go(Screen::Extra(Extra::Agents)));
            }
            let ag = if b.agenda_today > 0 { plural(b.agenda_today, "impegno oggi", "impegni oggi") } else { "Appuntamenti, attività, scadenze".into() };
            if menu_item(ui, icon::CALENDAR_BLANK, "Agenda", &ag, "") {
                nav.push(Nav::Go(Screen::Extra(Extra::Agenda)));
            }
        });
        if let Some((label, dirty)) = editor {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let t = if dirty { format!("{}  (non salvato)", label) } else { label };
                if top_button(ui, icon::PENCIL_SIMPLE, &t, cur == "editor", 0, false, |_| {}).on_hover_text("Torna al documento aperto").clicked() {
                    nav.push(Nav::Go(Screen::Editor));
                }
            });
        }
    });
}

/// Barra superiore: azienda e archivio, ricerca generale, strumenti.
pub fn top_bar(ui: &mut Ui, store: &Store, recent: &[PathBuf], search: &mut Search, pec_busy: bool, screen: Screen, nav: &mut Vec<Nav>) {
    let company = store.company().unwrap_or_default();
    let name = if company.display_name().is_empty() { "Azienda da configurare".to_string() } else { company.display_name() };
    let full = ui.available_width();
    ui.horizontal_centered(|ui| {
        ui.set_width(full);
        theme::mark(ui, 28.0);
        ui.add_space(2.0);
        let file = store.path().file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let comp = ui
            .scope(|ui| {
                theme::flat_visuals(ui, Color32::TRANSPARENT, RAISED, ACTIVE);
                let mut job = egui::text::LayoutJob::default();
                job.append(&name, 0.0, egui::TextFormat { font_id: FontId::new(15.0, theme::bold_family()), color: INK, valign: egui::Align::Center, ..Default::default() });
                job.append(icon::CARET_UP_DOWN, 6.0, egui::TextFormat { font_id: FontId::new(14.0, FontFamily::Proportional), color: MUTED, valign: egui::Align::Center, ..Default::default() });
                egui::menu::menu_custom_button(ui, egui::Button::new(job).min_size(Vec2::new(0.0, 36.0)), |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("   Archivio: {}", file)).size(12.0).color(MUTED));
                    ui.add_space(4.0);
                    if menu_item(ui, icon::GEAR_SIX, "Dati dell'azienda", "Ragione sociale, partita IVA, regime fiscale", "") {
                        nav.push(Nav::Go(Screen::Settings));
                    }
                    if menu_item(ui, icon::FOLDER_OPEN, "Apri un altro archivio...", "Ogni azienda ha il proprio file", "") {
                        if let Some(p) = rfd::FileDialog::new().add_filter("Archivio openFatt", &["gadb"]).pick_file() {
                            nav.push(Nav::OpenArchive(p));
                        }
                    }
                    let others: Vec<&PathBuf> = recent.iter().filter(|r| r.as_path() != store.path() && r.exists()).take(4).collect();
                    if !others.is_empty() {
                        menu_sep(ui);
                        for r in others {
                            let n = r.file_stem().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                            if menu_item(ui, icon::DATABASE, &n, &r.parent().map(|p| p.display().to_string()).unwrap_or_default(), "") {
                                nav.push(Nav::OpenArchive(r.clone()));
                            }
                        }
                    }
                    menu_sep(ui);
                    if menu_item(ui, icon::SIGN_OUT, "Chiudi archivio", "", "") {
                        nav.push(Nav::CloseArchive);
                    }
                })
                .response
            })
            .inner;
        comp.on_hover_text(store.path().display().to_string());

        // ricerca generale al centro
        let right_w = 300.0;
        let left_used = ui.min_rect().width();
        let field_w = (full * 0.34).clamp(260.0, 480.0);
        let gap = ((full - field_w) / 2.0 - left_used).max(12.0);
        ui.add_space(gap);
        global_search(ui, store, search, field_w, nav);

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.set_max_width(right_w);
            if theme::icon_button(ui, icon::DATABASE, "Archivio, copie di sicurezza, importazioni ed esportazioni").clicked() {
                nav.push(Nav::Go(Screen::Tools));
            }
            if theme::icon_button(ui, icon::GEAR_SIX, "Impostazioni").clicked() {
                nav.push(Nav::Go(Screen::Settings));
            }
            let _ = screen;
            let txt = if pec_busy { "Ricezione..." } else { "Ricevi PEC" };
            if theme::ghost(ui, !pec_busy, icon::ENVELOPE_SIMPLE, txt).on_hover_text("Scarica fatture e notifiche dello SdI dalla casella PEC (F5)").clicked() {
                nav.push(Nav::PecFetch);
            }
        });
    });
}

fn global_search(ui: &mut Ui, store: &Store, st: &mut Search, width: f32, nav: &mut Vec<Nav>) {
    let id = egui::Id::new("global_search");
    let frame = egui::Frame::new().fill(theme::SURFACE).stroke(Stroke::new(1.0_f32, theme::CONTROL)).corner_radius(6).inner_margin(egui::Margin::symmetric(10, 0));
    let resp = frame
        .show(ui, |ui| {
            ui.set_width(width - 20.0);
            ui.set_height(34.0);
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new(icon::MAGNIFYING_GLASS).size(16.0).color(MUTED));
                let te = egui::TextEdit::singleline(&mut st.text).id(id).frame(false).hint_text("Cerca clienti, articoli, documenti").desired_width(width - 100.0);
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
    let q = st.text.trim().to_string();
    if q.len() < 2 {
        return;
    }
    let parties = store.parties(&openfatt_store::PartyFilter { role: None, search: q.clone(), include_inactive: false }).unwrap_or_default();
    let products = store.products(&q, false).unwrap_or_default();
    let docs = store.documents(&openfatt_store::DocFilter { search: q.clone(), ..Default::default() }).unwrap_or_default();
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
pub fn status_bar(ui: &mut Ui, store: Option<&Store>, pec_busy: Option<&str>, pec_ok: bool) {
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        let small = |t: &str| RichText::new(t).size(12.0).color(MUTED);
        if let Some(s) = store {
            ui.label(small(&s.path().display().to_string()));
        } else {
            ui.label(small("Nessun archivio aperto"));
        }
        if let Some(b) = pec_busy {
            ui.spinner();
            ui.label(small(b));
        } else if store.is_some() {
            let (dot, t) = if pec_ok { (theme::POSITIVE, "PEC configurata") } else { (MUTED, "PEC da configurare in Impostazioni") };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, dot);
                ui.label(small(t));
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(small(&format!("openFatt {}", env!("CARGO_PKG_VERSION"))));
        });
    });
    let _ = ACCENT;
}
