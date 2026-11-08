//! Schermate e componenti dell'interfaccia.

pub mod agenda;
pub mod agents;
pub mod backup;
pub mod bank;
pub mod business;
pub mod cards;
pub mod conservation;
pub mod counter;
pub mod documents;
pub mod editor;
pub mod einvoices;
pub mod grid;
pub mod home;
pub mod inventory;
pub mod layout_editor;
pub mod linetools;
pub mod ledger;
pub mod mail;
pub mod parties;
pub mod pricelists;
pub mod print;
pub mod products;
pub mod quick;
pub mod settings;
pub mod shop;
pub mod stats;
pub mod shell;
pub mod signature;
pub mod stock;
pub mod theme;
pub mod tools;
pub mod tracking;
pub mod vat;
pub mod users;
pub mod widgets;
pub mod yearend;

use std::time::Instant;

use eframe::egui::{self, Color32, RichText};
use openfatt_core::model::{DocKind, PartyRole};
use openfatt_store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    Ok,
    Warn,
    Err,
}

pub struct Msg {
    pub kind: MsgKind,
    pub text: String,
    pub at: Instant,
}

#[derive(Default)]
pub struct Msgs {
    pub list: Vec<Msg>,
    /// messaggio lungo da mostrare in una finestra (es. elenco errori)
    pub dialog: Option<(String, String)>,
}

impl Msgs {
    pub fn ok(&mut self, t: impl Into<String>) {
        self.list.push(Msg { kind: MsgKind::Ok, text: t.into(), at: Instant::now() });
    }
    pub fn warn(&mut self, t: impl Into<String>) {
        self.list.push(Msg { kind: MsgKind::Warn, text: t.into(), at: Instant::now() });
    }
    pub fn err(&mut self, t: impl Into<String>) {
        let t = t.into();
        if t.lines().count() > 2 || t.len() > 160 {
            self.dialog = Some(("Attenzione".into(), t.clone()));
        }
        self.list.push(Msg { kind: MsgKind::Err, text: t.lines().next().unwrap_or("").to_string(), at: Instant::now() });
    }
    pub fn info_dialog(&mut self, title: &str, t: impl Into<String>) {
        self.dialog = Some((title.into(), t.into()));
    }
    /// Registra l'esito di un'operazione che può fallire.
    pub fn check<T, E: std::fmt::Display>(&mut self, r: Result<T, E>, ok: &str) -> Option<T> {
        match r {
            Ok(v) => {
                if !ok.is_empty() {
                    self.ok(ok);
                }
                Some(v)
            }
            Err(e) => {
                self.err(e.to_string());
                None
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        self.list.retain(|m| m.at.elapsed().as_secs() < if m.kind == MsgKind::Err { 9 } else { 5 });
        if !self.list.is_empty() {
            egui::Area::new(egui::Id::new("toasts")).anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -16.0]).show(ctx, |ui| {
                for m in self.list.iter().rev().take(5) {
                    let (bg, fg) = match m.kind {
                        MsgKind::Ok => (Color32::from_rgb(0xdc, 0xfa, 0xe5), Color32::from_rgb(0x04, 0x25, 0x0e)),
                        MsgKind::Warn => (Color32::from_rgb(0xff, 0xf4, 0xd6), Color32::from_rgb(0x92, 0x40, 0x0e)),
                        MsgKind::Err => (Color32::from_rgb(0xff, 0xed, 0xea), Color32::from_rgb(0x83, 0x1c, 0x1e)),
                    };
                    let ic = match m.kind {
                        MsgKind::Ok => theme::icon::CHECK_CIRCLE,
                        MsgKind::Warn => theme::icon::WARNING,
                        MsgKind::Err => theme::icon::WARNING_CIRCLE,
                    };
                    egui::Frame::new().fill(bg).corner_radius(8.0).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
                        ui.set_max_width(460.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(ic).size(18.0).color(fg));
                            ui.label(RichText::new(&m.text).color(fg));
                        });
                    });
                    ui.add_space(6.0);
                }
            });
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        let mut close = false;
        if let Some((title, text)) = &self.dialog {
            egui::Window::new(title.as_str()).collapsible(false).resizable(true).default_width(560.0).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    ui.label(text.as_str());
                });
                ui.separator();
                if ui.button("Chiudi").clicked() {
                    close = true;
                }
            });
        }
        if close {
            self.dialog = None;
        }
    }
}

/// Gruppi di documenti mostrati nel menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocGroup {
    Quotes,
    Orders,
    DeliveryNotes,
    Invoices,
    Proforma,
    SelfInvoices,
    SupplierQuotes,
    SupplierOrders,
    GoodsReceipts,
    PurchaseInvoices,
    Production,
}

impl DocGroup {
    pub const SALES: [DocGroup; 6] = [DocGroup::Quotes, DocGroup::Orders, DocGroup::DeliveryNotes, DocGroup::Invoices, DocGroup::Proforma, DocGroup::SelfInvoices];
    pub const PURCHASES: [DocGroup; 4] = [DocGroup::SupplierQuotes, DocGroup::SupplierOrders, DocGroup::GoodsReceipts, DocGroup::PurchaseInvoices];

    pub fn kinds(self) -> Vec<DocKind> {
        match self {
            DocGroup::Quotes => vec![DocKind::Quote],
            DocGroup::Orders => vec![DocKind::CustomerOrder],
            DocGroup::DeliveryNotes => vec![DocKind::DeliveryNote],
            DocGroup::Invoices => vec![DocKind::Invoice, DocKind::AccompanyingInvoice, DocKind::CreditNote, DocKind::DebitNote, DocKind::DownPaymentInvoice, DocKind::Fee],
            DocGroup::Proforma => vec![DocKind::Proforma],
            DocGroup::SelfInvoices => vec![DocKind::SelfInvoice],
            DocGroup::SupplierQuotes => vec![DocKind::SupplierQuote],
            DocGroup::SupplierOrders => vec![DocKind::SupplierOrder],
            DocGroup::GoodsReceipts => vec![DocKind::GoodsReceipt],
            DocGroup::PurchaseInvoices => vec![DocKind::PurchaseInvoice, DocKind::PurchaseCreditNote],
            DocGroup::Production => vec![DocKind::Production],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DocGroup::Quotes => "Preventivi",
            DocGroup::Orders => "Ordini clienti",
            DocGroup::DeliveryNotes => "DDT",
            DocGroup::Invoices => "Fatture e note",
            DocGroup::Proforma => "Proforma",
            DocGroup::SelfInvoices => "Autofatture / integrazioni",
            DocGroup::SupplierQuotes => "Preventivi fornitori",
            DocGroup::SupplierOrders => "Ordini fornitori",
            DocGroup::GoodsReceipts => "Arrivi merce",
            DocGroup::PurchaseInvoices => "Fatture di acquisto",
            DocGroup::Production => "Produzione",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Parties(PartyRole),
    Products,
    PriceLists,
    Documents(DocGroup),
    Counter,
    Editor,
    EInvoicesOut,
    EInvoicesIn,
    Dues,
    Cash,
    Bank,
    Vat,
    Stock,
    Inventory,
    Settings,
    Tools,
    Backup,
    /// Schermate delle funzioni avanzate (ricorrenti, agenti, statistiche, agenda...)
    Extra(business::Extra),
}

/// Richieste di navigazione o di lavoro prodotte dalle schermate.
pub enum Nav {
    Go(Screen),
    OpenDocument(i64),
    NewDocument(DocKind),
    EditDocument(openfatt_core::model::Document),
    CloseEditor,
    PecSend(i64),
    PecFetch,
    PecTest(openfatt_pec::PecConfig, String),
    OpenArchive(std::path::PathBuf),
    NewArchive(std::path::PathBuf),
    CloseArchive,
    /// L'archivio è stato modificato da strumenti esterni: ricaricare le schermate.
    Refresh,
    /// Apre l'anagrafica indicata (dalla ricerca generale).
    FindParty(PartyRole, i64, String),
    /// Apre l'articolo indicato (dalla ricerca generale).
    FindProduct(i64, String),
    /// Vendita al banco su una scheda precisa ("cassa", "vendite", "corrispettivi").
    CounterTab(&'static str),
    /// Apre la finestra di composizione con uno o più messaggi da inviare.
    Mail(Vec<crate::mailer::Compose>),
    /// Verifica della casella email.
    MailTest(openfatt_pec::mail::SmtpAccount, String),
    /// Scadenzario, scheda dei solleciti.
    Reminders,
    /// Scadenzario su una vista precisa (tutti, entrate, uscite, da saldare).
    Payments(ledger::PayView),
    /// Impostazioni, scheda email e solleciti.
    MailSettings,
    /// Impostazioni, scheda banca e distinte.
    BankSettings,
    /// Distinte bancarie su una scheda precisa.
    BankTab(bank::Tab),
    /// Registri e liquidazione IVA su una scheda precisa.
    VatTab(vat::Tab),
    /// Finestra delle etichette.
    Labels(Vec<crate::printing::LabelItem>),
    /// Impostazioni, scheda stampe.
    PrintSettings,
}

pub struct Ctx<'a> {
    pub store: &'a Store,
    pub msgs: &'a mut Msgs,
    pub nav: &'a mut Vec<Nav>,
}

/// Elenco letto dall'archivio e tenuto in memoria finché l'archivio non cambia (anche da
/// un'altra postazione) o non cambiano i filtri: con decine di migliaia di righe evita di
/// rileggere tutto a ogni ridisegno della finestra.
pub struct Cache<K, V> {
    key: Option<(u64, K)>,
    value: Option<std::rc::Rc<V>>,
    /// Versione da passare alla griglia: cambia quando cambiano i dati.
    pub version: u64,
}

impl<K, V> Default for Cache<K, V> {
    fn default() -> Self {
        Cache { key: None, value: None, version: 0 }
    }
}

impl<K: PartialEq + Clone, V> Cache<K, V> {
    pub fn get(&mut self, generation: u64, key: &K, load: impl FnOnce() -> V) -> std::rc::Rc<V> {
        let fresh = matches!(&self.key, Some((g, k)) if *g == generation && k == key);
        if !fresh || self.value.is_none() {
            self.value = Some(std::rc::Rc::new(load()));
            self.key = Some((generation, key.clone()));
            self.version = self.version.wrapping_add(1).max(1);
        }
        self.value.clone().unwrap()
    }
}

/// Area dei permessi a cui appartiene una schermata.
pub fn screen_area(s: Screen) -> openfatt_store::Area {
    use business::Extra;
    use openfatt_store::Area;
    match s {
        Screen::Home | Screen::Editor => Area::Sales,
        Screen::Parties(_) | Screen::Products | Screen::PriceLists => Area::Registry,
        Screen::Counter => Area::Counter,
        Screen::Documents(DocGroup::Production) => Area::Stock,
        Screen::Documents(g) if DocGroup::PURCHASES.contains(&g) => Area::Purchases,
        Screen::Documents(_) => Area::Sales,
        Screen::EInvoicesOut | Screen::EInvoicesIn => Area::EInvoice,
        Screen::Stock | Screen::Inventory => Area::Stock,
        Screen::Dues | Screen::Cash | Screen::Bank | Screen::Vat => Area::Accounting,
        Screen::Settings | Screen::Tools | Screen::Backup => Area::Settings,
        Screen::Extra(x) => match x {
            Extra::Recurring | Extra::Shop => Area::Sales,
            Extra::Cards => Area::Counter,
            Extra::Agents | Extra::Stats | Extra::Agenda => Area::Analysis,
            Extra::YearEnd => Area::Settings,
            Extra::Conservation => Area::EInvoice,
        },
    }
}

/// Area dei permessi di un tipo di documento.
pub fn kind_area(k: DocKind) -> openfatt_store::Area {
    use openfatt_store::Area;
    if k.is_purchase() {
        Area::Purchases
    } else if k.is_internal() {
        Area::Stock
    } else if k == DocKind::RetailSale {
        Area::Counter
    } else {
        Area::Sales
    }
}

/// Titolo della schermata: la barra della sezione.
pub fn title(ui: &mut egui::Ui, t: &str) {
    theme::title_row(ui, &[], t, None, |_| {});
}
