//! Scrivania: una bacheca di schede (riepiloghi, attività, scorciatoie, post-it) che
//! l'utente sistema come preferisce con il pulsante "Personalizza".
//!
//! Le schede stanno su una griglia di 4 colonne (2 o 1 con finestre strette): ognuna
//! occupa 1, 2 o 4 colonne, si sposta trascinandola dalla maniglia, si nasconde e prende
//! il colore scelto. I dati si leggono una volta sola per ogni cambio dell'archivio.

use chrono::{Datelike, NaiveDate, Timelike};
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Sense, Stroke, Ui, Vec2};
use openfatt_core::model::{DocKind, DocStatus, PartyRole};
use openfatt_store::{Direction, EInvoiceStatus};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::theme::{self, icon, Section, Tone, MUTED, NEGATIVE};
use super::widgets::eur;
use super::{Ctx, DocGroup, Nav, Screen};

// ============================================================================ stato

pub struct State {
    issues: Option<(String, Vec<openfatt_fatturapa::Issue>)>,
    data: super::Cache<NaiveDate, HomeData>,
    /// Schede nell'ordine in cui compaiono.
    pub cards: Vec<Slot>,
    /// Scorciatoie (codici del catalogo `SHORTCUTS`).
    pub shortcuts: Vec<String>,
    pub notes: Vec<Note>,
    /// Modalità "Personalizza".
    pub editing: bool,
    /// Elenco "Da fare" aperto per intero.
    todo_all: bool,
    /// Post-it appena creato, a cui dare il cursore.
    focus_note: Option<u64>,
    /// Archivio e utente per cui è stata letta la disposizione.
    loaded: Option<(std::path::PathBuf, String)>,
    /// Ultima disposizione salvata e momento dell'ultima modifica non salvata.
    saved: Option<Prefs>,
    dirty_at: Option<f64>,
    /// Bacheca di dimostrazione: non legge e non salva nulla.
    demo: bool,
}

/// Disposizione della scrivania salvata nell'archivio (per utente).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Prefs {
    cards: Vec<SlotPref>,
    /// `None` = scorciatoie predefinite (lista vuota = nessuna scorciatoia).
    shortcuts: Option<Vec<String>>,
    notes: Vec<NotePref>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SlotPref {
    code: String,
    span: u8,
    #[serde(default)]
    accent: Option<u8>,
    #[serde(default)]
    hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct NotePref {
    id: u64,
    text: String,
    #[serde(default)]
    color: u8,
    #[serde(default)]
    done: bool,
    date: NaiveDate,
}

const PREF: &str = "scrivania";

impl Default for State {
    fn default() -> Self {
        State {
            issues: None,
            data: Default::default(),
            cards: default_cards(),
            shortcuts: DEFAULT_SHORTCUTS.iter().map(|s| s.to_string()).collect(),
            notes: Vec::new(),
            editing: false,
            todo_all: false,
            focus_note: None,
            loaded: None,
            saved: None,
            dirty_at: None,
            demo: false,
        }
    }
}

impl State {
    fn prefs(&self) -> Prefs {
        let defaults: Vec<String> = DEFAULT_SHORTCUTS.iter().map(|s| s.to_string()).collect();
        Prefs {
            cards: if self.cards == default_cards() {
                Vec::new()
            } else {
                self.cards.iter().map(|c| SlotPref { code: c.kind.code().to_string(), span: c.span, accent: c.accent, hidden: c.hidden }).collect()
            },
            shortcuts: if self.shortcuts == defaults { None } else { Some(self.shortcuts.clone()) },
            notes: self.notes.iter().map(|n| NotePref { id: n.id, text: n.text.clone(), color: n.color, done: n.done, date: n.date }).collect(),
        }
    }

    fn set_prefs(&mut self, p: &Prefs) {
        let mut cards: Vec<Slot> = Vec::new();
        for c in &p.cards {
            if let Some(kind) = Card::ALL.into_iter().find(|k| k.code() == c.code) {
                if !cards.iter().any(|x| x.kind == kind) {
                    let span = if kind.spans().contains(&c.span) { c.span } else { kind.spans()[0] };
                    cards.push(Slot { kind, span, accent: c.accent.filter(|a| (*a as usize) < ACCENTS.len()), hidden: c.hidden });
                }
            }
        }
        // schede arrivate con versioni successive: in fondo, visibili
        for d in default_cards() {
            if !cards.iter().any(|x| x.kind == d.kind) {
                cards.push(d);
            }
        }
        self.cards = if p.cards.is_empty() { default_cards() } else { cards };
        self.shortcuts = match &p.shortcuts {
            Some(l) => l.iter().filter(|c| shortcut(c).is_some()).cloned().collect(),
            None => DEFAULT_SHORTCUTS.iter().map(|s| s.to_string()).collect(),
        };
        self.notes = p.notes.iter().map(|n| Note { id: n.id, text: n.text.clone(), color: n.color % PAPERS.len() as u8, done: n.done, date: n.date }).collect();
    }

    /// Rilegge la disposizione quando cambiano archivio o utente.
    fn sync(&mut self, store: &openfatt_store::Store) {
        if self.demo {
            return;
        }
        let key = Some((store.path().to_path_buf(), store.current_user()));
        if key != self.loaded {
            let p: Prefs = store.user_pref(PREF).unwrap_or_default();
            self.set_prefs(&p);
            self.saved = Some(self.prefs());
            self.loaded = key;
            self.dirty_at = None;
            self.editing = false;
        }
    }

    /// Salva poco dopo l'ultima modifica: scrivendo un post-it non si salva a ogni lettera
    /// (ogni salvataggio fa rileggere gli elenchi a tutte le schermate).
    fn persist(&mut self, ctx: &egui::Context, store: &openfatt_store::Store, msgs: &mut super::Msgs) {
        if self.demo || self.loaded.is_none() {
            return;
        }
        let now = self.prefs();
        if self.saved.as_ref() == Some(&now) {
            self.dirty_at = None;
            return;
        }
        let t = ctx.input(|i| i.time);
        // ogni nuova modifica fa ripartire l'attesa
        let edited = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(_) | egui::Event::Key { .. } | egui::Event::PointerButton { pressed: false, .. })));
        if edited || self.dirty_at.is_none() {
            self.dirty_at = Some(t);
        }
        let since = self.dirty_at.unwrap_or(t);
        // con il cursore in un post-it si aspetta una pausa più lunga
        let typing = ctx.memory(|m| m.focused().is_some());
        let wait = if typing { 2.0 } else { 0.6 };
        if t - since >= wait {
            if let Err(e) = store.set_user_pref(PREF, &now) {
                msgs.err(format!("Impossibile salvare la scrivania: {}", e));
            }
            self.saved = Some(now);
            self.dirty_at = None;
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait - (t - since) + 0.05));
        }
    }

    /// Salva subito le modifiche in sospeso (chiusura dell'archivio o del programma).
    pub fn flush(&mut self, store: &openfatt_store::Store) {
        if self.demo || self.loaded.as_ref().map(|(p, u)| (p.as_path(), u.as_str())) != Some((store.path(), store.current_user().as_str())) {
            return;
        }
        let now = self.prefs();
        if self.saved.as_ref() != Some(&now) && store.set_user_pref(PREF, &now).is_ok() {
            self.saved = Some(now);
        }
    }

    /// Bacheca di dimostrazione (anteprime e schermate di prova).
    pub fn demo(editing: bool) -> Self {
        let mut s = State { editing, demo: true, ..Default::default() };
        let today = chrono::Local::now().date_naive();
        for (i, (t, c)) in [
            ("Chiamare Rossi Arredamenti per il saldo della fattura 1", 0u8),
            ("Ordinare toner e carta per la stampante", 2),
            ("Venerdì: portare le Ri.Ba. in banca", 3),
        ]
        .iter()
        .enumerate()
        {
            s.notes.push(Note { id: i as u64 + 1, text: t.to_string(), color: *c, done: false, date: today });
        }
        s
    }
}

/// Una scheda della bacheca.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub kind: Card,
    /// Colonne occupate: 1, 2 o 4.
    pub span: u8,
    /// Colore scelto (indice di `ACCENTS`); `None` = colore della sezione.
    pub accent: Option<u8>,
    pub hidden: bool,
}

/// Un post-it.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub id: u64,
    pub text: String,
    /// Indice di `PAPERS`.
    pub color: u8,
    pub done: bool,
    pub date: NaiveDate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Card {
    Receivable,
    Overdue,
    Revenue,
    Payables,
    Todo,
    Shortcuts,
    Notes,
    Chart,
    Incoming,
    Agenda,
    EInvoices,
    Stock,
    Recent,
}

impl Card {
    pub const ALL: [Card; 13] = [
        Card::Receivable,
        Card::Overdue,
        Card::Revenue,
        Card::Payables,
        Card::Todo,
        Card::Shortcuts,
        Card::Notes,
        Card::Chart,
        Card::Incoming,
        Card::Agenda,
        Card::EInvoices,
        Card::Stock,
        Card::Recent,
    ];

    /// Codice stabile (per salvare la disposizione).
    pub fn code(self) -> &'static str {
        match self {
            Card::Receivable => "da_incassare",
            Card::Overdue => "scaduti",
            Card::Revenue => "fatturato_anno",
            Card::Payables => "da_pagare",
            Card::Todo => "da_fare",
            Card::Shortcuts => "scorciatoie",
            Card::Notes => "post_it",
            Card::Chart => "grafico_fatturato",
            Card::Incoming => "incassi_in_arrivo",
            Card::Agenda => "agenda",
            Card::EInvoices => "fatture_elettroniche",
            Card::Stock => "sotto_scorta",
            Card::Recent => "ultimi_documenti",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Card::Receivable => "Da incassare",
            Card::Overdue => "Crediti scaduti",
            Card::Revenue => "Fatturato dell'anno",
            Card::Payables => "Da pagare ai fornitori",
            Card::Todo => "Da fare",
            Card::Shortcuts => "Scorciatoie",
            Card::Notes => "Post-it",
            Card::Chart => "Fatturato per mese",
            Card::Incoming => "Incassi in arrivo",
            Card::Agenda => "Prossimi impegni",
            Card::EInvoices => "Fatture elettroniche",
            Card::Stock => "Sotto scorta",
            Card::Recent => "Ultimi documenti",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Card::Receivable => icon::HAND_COINS,
            Card::Overdue => icon::CLOCK_COUNTDOWN,
            Card::Revenue => icon::TREND_UP,
            Card::Payables => icon::WALLET,
            Card::Todo => icon::LIST_CHECKS,
            Card::Shortcuts => icon::LIGHTNING,
            Card::Notes => icon::NOTE,
            Card::Chart => icon::CHART_BAR,
            Card::Incoming => icon::CALENDAR_CHECK,
            Card::Agenda => icon::CALENDAR_DOTS,
            Card::EInvoices => icon::PAPER_PLANE_TILT,
            Card::Stock => icon::PACKAGE,
            Card::Recent => icon::FILES,
        }
    }

    /// Sezione da cui prende il colore se l'utente non ne sceglie un altro.
    fn section(self) -> Section {
        match self {
            Card::Receivable | Card::Overdue | Card::Payables | Card::Incoming => Section::Payments,
            Card::Revenue | Card::Chart => Section::Analysis,
            Card::Todo | Card::Shortcuts => Section::Start,
            Card::Notes => Section::Stock,
            Card::Agenda => Section::Agenda,
            Card::EInvoices => Section::EInvoice,
            Card::Stock => Section::Stock,
            Card::Recent => Section::Sales,
        }
    }

    /// Larghezze possibili (colonne).
    fn spans(self) -> &'static [u8] {
        match self {
            Card::Receivable | Card::Overdue | Card::Revenue | Card::Payables => &[1, 2],
            Card::Incoming | Card::Agenda | Card::EInvoices | Card::Stock => &[1, 2],
            Card::Todo | Card::Recent => &[2, 4],
            Card::Shortcuts | Card::Notes | Card::Chart => &[1, 2, 4],
        }
    }

    fn kpi(self) -> bool {
        matches!(self, Card::Receivable | Card::Overdue | Card::Revenue | Card::Payables)
    }
}

pub fn default_cards() -> Vec<Slot> {
    let s = |kind, span| Slot { kind, span, accent: None, hidden: false };
    vec![
        s(Card::Receivable, 1),
        s(Card::Overdue, 1),
        s(Card::Revenue, 1),
        s(Card::Payables, 1),
        s(Card::Shortcuts, 4),
        s(Card::Todo, 2),
        s(Card::Incoming, 1),
        s(Card::Agenda, 1),
        s(Card::Chart, 2),
        s(Card::Notes, 2),
        s(Card::EInvoices, 1),
        s(Card::Stock, 1),
        s(Card::Recent, 2),
    ]
}

/// Colori che si possono dare a una scheda.
const ACCENTS: [(u8, u8, u8, &str); 10] = [
    (0x36, 0x4f, 0xc7, "Indaco"),
    (0x19, 0x71, 0xc2, "Blu"),
    (0x0c, 0x85, 0x99, "Ottanio"),
    (0x08, 0x7f, 0x5b, "Verde bosco"),
    (0x2f, 0x7d, 0x32, "Verde"),
    (0xa8, 0x5d, 0x00, "Ambra"),
    (0xc2, 0x41, 0x0c, "Arancio"),
    (0xc2, 0x25, 0x5c, "Lampone"),
    (0x86, 0x2e, 0x9c, "Viola"),
    (0x49, 0x5a, 0x6e, "Ardesia"),
];

fn accent_color(i: u8) -> Color32 {
    let (r, g, b, _) = ACCENTS[(i as usize).min(ACCENTS.len() - 1)];
    Color32::from_rgb(r, g, b)
}

/// Carta dei post-it: (fondo, nome).
const PAPERS: [((u8, u8, u8), &str); 5] = [
    ((0xff, 0xf1, 0xa8), "Giallo"),
    ((0xff, 0xd9, 0xe3), "Rosa"),
    ((0xd4, 0xf4, 0xd8), "Verde"),
    ((0xd6, 0xe8, 0xff), "Azzurro"),
    ((0xff, 0xe2, 0xc4), "Arancio"),
];

fn paper(i: u8) -> Color32 {
    let ((r, g, b), _) = PAPERS[(i as usize) % PAPERS.len()];
    Color32::from_rgb(r, g, b)
}

// ============================================================================ scorciatoie

/// Funzione che si può mettere tra le scorciatoie.
struct Shortcut {
    code: &'static str,
    icon: &'static str,
    label: &'static str,
    section: Section,
}

const SHORTCUTS: &[Shortcut] = &[
    Shortcut { code: "nuova_fattura", icon: icon::FILE_PLUS, label: "Nuova fattura", section: Section::Sales },
    Shortcut { code: "nuovo_preventivo", icon: icon::NOTE_PENCIL, label: "Nuovo preventivo", section: Section::Sales },
    Shortcut { code: "nuovo_ordine", icon: icon::CLIPBOARD_TEXT, label: "Ordine cliente", section: Section::Sales },
    Shortcut { code: "nuovo_ddt", icon: icon::TRUCK, label: "Nuovo DDT", section: Section::Sales },
    Shortcut { code: "nota_credito", icon: icon::RECEIPT, label: "Nota di credito", section: Section::Sales },
    Shortcut { code: "fattura_acquisto", icon: icon::SHOPPING_CART, label: "Fattura di acquisto", section: Section::Purchases },
    Shortcut { code: "banco", icon: icon::CASH_REGISTER, label: "Vendita al banco", section: Section::Counter },
    Shortcut { code: "registra_incasso", icon: icon::HAND_COINS, label: "Registra incasso", section: Section::Payments },
    Shortcut { code: "scadenzario", icon: icon::CALENDAR_CHECK, label: "Scadenzario", section: Section::Payments },
    Shortcut { code: "solleciti", icon: icon::ENVELOPE_SIMPLE, label: "Solleciti", section: Section::Payments },
    Shortcut { code: "prima_nota", icon: icon::BOOK_OPEN, label: "Prima nota", section: Section::Payments },
    Shortcut { code: "fatturazione_differita", icon: icon::FILES, label: "Fatturazione DDT", section: Section::Sales },
    Shortcut { code: "fe_inviate", icon: icon::PAPER_PLANE_TILT, label: "Fatture inviate", section: Section::EInvoice },
    Shortcut { code: "fe_ricevute", icon: icon::TRAY_ARROW_DOWN, label: "Fatture ricevute", section: Section::EInvoice },
    Shortcut { code: "clienti", icon: icon::USERS, label: "Clienti", section: Section::Customers },
    Shortcut { code: "fornitori", icon: icon::TRUCK, label: "Fornitori", section: Section::Suppliers },
    Shortcut { code: "articoli", icon: icon::PACKAGE, label: "Articoli", section: Section::Products },
    Shortcut { code: "magazzino", icon: icon::WAREHOUSE, label: "Magazzino", section: Section::Stock },
    Shortcut { code: "liquidazione_iva", icon: icon::PERCENT, label: "Liquidazione IVA", section: Section::Tools },
    Shortcut { code: "agenda", icon: icon::CALENDAR_BLANK, label: "Agenda", section: Section::Agenda },
    Shortcut { code: "statistiche", icon: icon::CHART_LINE_UP, label: "Statistiche", section: Section::Analysis },
    Shortcut { code: "copia_sicurezza", icon: icon::CLOUD_ARROW_UP, label: "Copia di sicurezza", section: Section::Archives },
];

const DEFAULT_SHORTCUTS: [&str; 8] = ["nuova_fattura", "nuovo_preventivo", "nuovo_ddt", "banco", "registra_incasso", "scadenzario", "fe_ricevute", "clienti"];

fn shortcut(code: &str) -> Option<&'static Shortcut> {
    SHORTCUTS.iter().find(|s| s.code == code)
}

fn shortcut_nav(code: &str) -> Option<Nav> {
    use super::business::Extra;
    Some(match code {
        "nuova_fattura" => Nav::NewDocument(DocKind::Invoice),
        "nuovo_preventivo" => Nav::NewDocument(DocKind::Quote),
        "nuovo_ordine" => Nav::NewDocument(DocKind::CustomerOrder),
        "nuovo_ddt" => Nav::NewDocument(DocKind::DeliveryNote),
        "nota_credito" => Nav::NewDocument(DocKind::CreditNote),
        "fattura_acquisto" => Nav::NewDocument(DocKind::PurchaseInvoice),
        "banco" => Nav::Go(Screen::Counter),
        "registra_incasso" => Nav::Payments(super::ledger::PayView::Register),
        "scadenzario" => Nav::Go(Screen::Dues),
        "solleciti" => Nav::Reminders,
        "prima_nota" => Nav::Go(Screen::Cash),
        "fatturazione_differita" => Nav::DeferredInvoicing,
        "fe_inviate" => Nav::Go(Screen::EInvoicesOut),
        "fe_ricevute" => Nav::Go(Screen::EInvoicesIn),
        "clienti" => Nav::Go(Screen::Parties(PartyRole::Customer)),
        "fornitori" => Nav::Go(Screen::Parties(PartyRole::Supplier)),
        "articoli" => Nav::Go(Screen::Products),
        "magazzino" => Nav::Go(Screen::Stock),
        "liquidazione_iva" => Nav::VatTab(super::vat::Tab::Liquidation),
        "agenda" => Nav::Go(Screen::Extra(Extra::Agenda)),
        "statistiche" => Nav::Go(Screen::Extra(Extra::Stats)),
        "copia_sicurezza" => Nav::Go(Screen::Backup),
        _ => return None,
    })
}

// ============================================================================ dati

/// Dati della scrivania, riletti solo quando l'archivio cambia.
struct HomeData {
    out: Vec<openfatt_store::EInvoice>,
    incoming: Vec<openfatt_store::EInvoice>,
    without_fe: Vec<i64>,
    dues: Vec<openfatt_store::DueRow>,
    under_min: Vec<openfatt_store::StockRow>,
    open_inv: Option<openfatt_store::InventorySession>,
    recent_docs: Vec<openfatt_store::DocSummary>,
    year_docs: Vec<openfatt_store::DocSummary>,
}

/// Tutto quello che le schede mostrano, calcolato una volta per ridisegno.
struct Facts {
    today: NaiveDate,
    todo: Vec<Todo>,
    receivables: Vec<openfatt_store::DueRow>,
    open_sum: Decimal,
    overdue_sum: Decimal,
    overdue_count: usize,
    oldest_overdue_days: i64,
    in30: Decimal,
    payables: Decimal,
    payables_overdue: Decimal,
    payables_count: usize,
    year_total: Decimal,
    month_labels: Vec<String>,
    month_values: Vec<f64>,
    rejected: usize,
    ready: usize,
    sent: usize,
    to_generate: usize,
    to_register: usize,
    agenda: Vec<openfatt_store::AgendaItem>,
    agenda_late: usize,
    under_min: Vec<openfatt_store::StockRow>,
    recent: Vec<openfatt_store::DocSummary>,
}

/// Una riga dell'elenco "Da fare".
struct Todo {
    icon: &'static str,
    bad: bool,
    title: String,
    desc: String,
    amount: Option<Decimal>,
    action_icon: &'static str,
    action: &'static str,
    go: Go,
}

enum Go {
    Nav(Nav),
    Generate(i64),
}

const WEEKDAYS: [&str; 7] = ["Lunedì", "Martedì", "Mercoledì", "Giovedì", "Venerdì", "Sabato", "Domenica"];
const MONTHS: [&str; 12] = ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"];
const MONTHS_SHORT: [&str; 12] = ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"];

fn long_date(d: NaiveDate) -> String {
    format!("{} {} {} {}", WEEKDAYS[d.weekday().num_days_from_monday() as usize], d.day(), MONTHS[d.month0() as usize], d.year())
}

fn sales_sign(k: DocKind) -> Option<Decimal> {
    match k {
        DocKind::Invoice | DocKind::AccompanyingInvoice | DocKind::DebitNote | DocKind::Fee | DocKind::DownPaymentInvoice | DocKind::RetailSale => Some(Decimal::ONE),
        DocKind::CreditNote => Some(Decimal::NEGATIVE_ONE),
        _ => None,
    }
}

fn gather(ui: &Ui, s: &openfatt_store::Store, st: &mut State, today: NaiveDate) -> Facts {
    let company = s.company().unwrap_or_default();
    let first_month = {
        let m = today.month0() as i32 - 5;
        let (y, m) = if m < 0 { (today.year() - 1, m + 12) } else { (today.year(), m) };
        NaiveDate::from_ymd_opt(y, m as u32 + 1, 1).unwrap()
    };
    let hd = st.data.get(s.generation(), &today, || HomeData {
        out: s.einvoices(Direction::Out, "", None).unwrap_or_default(),
        incoming: s.einvoices(Direction::In, "", None).unwrap_or_default().into_iter().filter(|e| e.status == EInvoiceStatus::Received).collect(),
        without_fe: s.documents_without_einvoice().unwrap_or_default(),
        dues: s.due_list(true, None).unwrap_or_default(),
        under_min: s.stock_report("", None).unwrap_or_default().into_iter().filter(|r| r.under_min).collect(),
        open_inv: s.inventory_sessions().unwrap_or_default().into_iter().find(|x| x.status == openfatt_store::InventoryStatus::Open),
        recent_docs: s.documents(&openfatt_store::DocFilter { from: Some(first_month), ..Default::default() }).unwrap_or_default(),
        year_docs: s.documents(&openfatt_store::DocFilter { year: Some(today.year()), status: Some(DocStatus::Issued), ..Default::default() }).unwrap_or_default(),
    });
    let out = &hd.out;
    let rejected: Vec<_> = out.iter().filter(|e| e.status == EInvoiceStatus::Rejected).collect();
    let ready: Vec<_> = out.iter().filter(|e| e.status == EInvoiceStatus::ToSend).collect();
    let sent = out.iter().filter(|e| e.status == EInvoiceStatus::Sent).count();
    let incoming = &hd.incoming;
    let without_fe = &hd.without_fe;
    let dues = &hd.dues;
    let mut receivables: Vec<openfatt_store::DueRow> = dues.iter().filter(|d| d.customer_side() && !d.is_credit() && d.residual() > Decimal::ZERO).cloned().collect();
    receivables.sort_by_key(|d| d.date);
    // note di credito ai clienti ancora da compensare o rimborsare: riducono il da incassare
    let open_credits: Decimal = dues.iter().filter(|d| d.customer_side() && d.is_credit()).map(|d| d.residual()).sum();
    let overdue: Vec<_> = receivables.iter().filter(|d| d.date < today).collect();
    let overdue_sum: Decimal = overdue.iter().map(|d| d.residual()).sum();
    let open_sum: Decimal = receivables.iter().map(|d| d.residual()).sum::<Decimal>() - open_credits;
    let in30: Decimal = receivables.iter().filter(|d| d.date >= today && d.date <= today + chrono::Duration::days(30)).map(|d| d.residual()).sum();
    let payable_rows: Vec<_> = dues.iter().filter(|d| !d.customer_side() && !d.is_credit() && d.residual() > Decimal::ZERO).collect();
    let payables_overdue: Decimal = payable_rows.iter().filter(|d| d.date < today).map(|d| d.residual()).sum();
    let supplier_credits: Decimal = dues.iter().filter(|d| !d.customer_side() && d.is_credit()).map(|d| d.residual()).sum();
    let payables: Decimal = payable_rows.iter().map(|d| d.residual()).sum::<Decimal>() - supplier_credits;
    let under_min = &hd.under_min;
    let open_inv = &hd.open_inv;
    let recent_docs = &hd.recent_docs;
    let drafts: Vec<_> = recent_docs.iter().filter(|d| d.status == DocStatus::Draft).collect();

    let mut todo: Vec<Todo> = Vec::new();
    if company.vat_number.is_empty() {
        todo.push(Todo {
            icon: icon::BUILDINGS,
            bad: true,
            title: "Completa i dati dell'azienda".into(),
            desc: "Ragione sociale, partita IVA, sede e regime fiscale servono per emettere fatture".into(),
            amount: None,
            action_icon: icon::GEAR_SIX,
            action: "Apri impostazioni",
            go: Go::Nav(Nav::Go(Screen::Settings)),
        });
    }
    for e in &rejected {
        todo.push(Todo {
            icon: icon::WARNING_CIRCLE,
            bad: true,
            title: format!("Fattura {} scartata dallo SdI", e.doc_number),
            desc: format!("{} · {}", e.party_name, first_line(&e.last_message)),
            amount: Some(e.total),
            action_icon: icon::ARROW_CLOCKWISE,
            action: "Correggi e reinvia",
            go: Go::Nav(e.document_id.map(Nav::OpenDocument).unwrap_or(Nav::Go(Screen::EInvoicesOut))),
        });
    }
    // tutto quello che segue si legge dall'archivio solo quando cambia (non a ogni ridisegno)
    let gen = s.generation();
    let ctx = ui.ctx().clone();
    let (to_remind, _) = super::memo(&ctx, "home_reminders", gen, today, || {
        let tpl = crate::mailer::templates(s);
        s.reminder_candidates(today, tpl.grace_days, tpl.min_days_between).unwrap_or_default().into_iter().filter(|c| !c.recently_reminded).collect::<Vec<_>>()
    });
    if !to_remind.is_empty() {
        let n = to_remind.len();
        let names: Vec<String> = to_remind.iter().take(3).map(|c| c.party_name.clone()).collect();
        todo.push(Todo {
            icon: icon::ENVELOPE_SIMPLE,
            bad: true,
            title: if n == 1 { "1 cliente da sollecitare".to_string() } else { format!("{} clienti da sollecitare", n) },
            desc: format!("{}{}", names.join(", "), if n > 3 { format!(" e altri {}", n - 3) } else { String::new() }),
            amount: Some(to_remind.iter().map(|c| c.overdue_total).sum()),
            action_icon: icon::PAPER_PLANE_TILT,
            action: "Prepara solleciti",
            go: Go::Nav(Nav::Reminders),
        });
    }
    // versamento IVA dell'ultimo periodo chiuso, se la scadenza è vicina
    if let Ok(vs) = super::memo(&ctx, "vat_settings", gen, (), || s.vat_settings().map_err(|e| e.to_string())).0.as_ref() {
        let cur = openfatt_core::vat::Period::containing(today, vs.periodicity);
        let last = if cur.index > 1 { openfatt_core::vat::Period { index: cur.index - 1, ..cur } } else { openfatt_core::vat::Period::new(cur.year - 1, cur.count(), vs.periodicity) };
        let due = last.payment_due();
        if due >= today - chrono::Duration::days(60) && due <= today + chrono::Duration::days(12) {
            let (liq, _) = super::memo(&ctx, "home_vat", gen, (last.year, last.index), || s.vat_liquidations(last.year, last.index).ok().and_then(|y| y.into_iter().last()));
            if let Some((_, l)) = liq.as_ref() {
                if l.pay_now && l.inputs.paid_on.is_none() {
                    todo.push(Todo {
                        icon: icon::PERCENT,
                        bad: due < today,
                        title: format!("IVA di {} da versare", last.label()),
                        desc: format!("entro il {} con F24, codice tributo {}", due.format("%d/%m/%Y"), last.f24_code()),
                        amount: Some(l.to_pay),
                        action_icon: icon::CALCULATOR,
                        action: "Liquidazione",
                        go: Go::Nav(Nav::VatTab(super::vat::Tab::Liquidation)),
                    });
                }
            }
        }
    }
    // distinte bancarie: Ri.Ba./SDD in scadenza entro 30 giorni, bonifici entro 7
    for (kind, days, tab, title, action) in [
        (openfatt_store::BankKind::Riba, 30, super::bank::Tab::Riba, "Ri.Ba. da presentare in banca", "Crea distinta"),
        (openfatt_store::BankKind::Sdd, 30, super::bank::Tab::Sdd, "Addebiti SDD da presentare", "Crea distinta"),
        (openfatt_store::BankKind::Sct, 7, super::bank::Tab::Sct, "Bonifici a fornitori da disporre", "Prepara bonifici"),
    ] {
        let (list, _) = super::memo(&ctx, ("home_bank", kind), gen, today, || s.bank_pending(kind).unwrap_or_default().into_iter().filter(|d| d.date <= today + chrono::Duration::days(days)).collect::<Vec<_>>());
        if !list.is_empty() {
            let first = list.iter().map(|d| d.date).min().unwrap();
            todo.push(Todo {
                icon: if kind == openfatt_store::BankKind::Sct { icon::ARROW_SQUARE_UP_RIGHT } else { icon::BANK },
                bad: first < today,
                title: format!("{} {}", list.len(), title),
                desc: format!("{} · prima scadenza {}", list.iter().map(|d| d.party_name.clone()).collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>().join(", "), first.format("%d/%m/%Y")),
                amount: Some(list.iter().map(|d| d.residual()).sum()),
                action_icon: icon::FILE_ARROW_DOWN,
                action,
                go: Go::Nav(Nav::BankTab(tab)),
            });
        }
    }
    for d in overdue.iter().take(3) {
        let days = (today - d.date).num_days();
        todo.push(Todo {
            icon: icon::CLOCK_COUNTDOWN,
            bad: true,
            title: format!("{} scaduta da {} {}", d.doc_label, days, if days == 1 { "giorno" } else { "giorni" }),
            desc: format!("{} · scadenza {}", d.party_name, d.date.format("%d/%m/%Y")),
            amount: Some(d.residual()),
            action_icon: icon::HAND_COINS,
            action: "Registra incasso",
            go: Go::Nav(Nav::PayDue(d.id)),
        });
    }
    if overdue.len() > 3 {
        todo.push(Todo {
            icon: icon::CLOCK_COUNTDOWN,
            bad: true,
            title: format!("Altre {} scadenze non incassate", overdue.len() - 3),
            desc: "Scadenze passate dei clienti".into(),
            amount: Some(overdue.iter().skip(3).map(|d| d.residual()).sum()),
            action_icon: icon::CALENDAR_CHECK,
            action: "Scadenzario",
            go: Go::Nav(Nav::Go(Screen::Dues)),
        });
    }
    let first_ids: Vec<i64> = without_fe.iter().take(4).copied().collect();
    let (without_docs, _) = super::memo(&ctx, "home_without_fe", gen, first_ids.clone(), || first_ids.iter().filter_map(|id| s.document(*id).ok()).collect::<Vec<_>>());
    for d in without_docs.iter() {
        todo.push(Todo {
            icon: icon::PAPER_PLANE_TILT,
            bad: false,
            title: format!("{} {} da trasmettere allo SdI", d.kind.label(), d.number_label()),
            desc: format!("{} · emessa il {}", d.party.display_name(), d.date.format("%d/%m/%Y")),
            amount: openfatt_core::calc::compute(d).ok().map(|t| t.total),
            action_icon: icon::FILE_ARROW_UP,
            action: "Genera XML",
            go: Go::Generate(d.id),
        });
    }
    for e in ready.iter().take(4) {
        todo.push(Todo {
            icon: icon::PAPER_PLANE_TILT,
            bad: false,
            title: format!("Fattura {}: XML pronto da inviare", e.doc_number),
            desc: format!("{} · {}", e.party_name, e.file_name),
            amount: Some(e.total),
            action_icon: icon::PAPER_PLANE_TILT,
            action: "Invia allo SdI",
            go: Go::Nav(Nav::PecSend(e.id)),
        });
    }
    if !incoming.is_empty() {
        let names: Vec<String> = incoming.iter().take(3).map(|e| e.party_name.clone()).collect();
        todo.push(Todo {
            icon: icon::TRAY_ARROW_DOWN,
            bad: false,
            title: if incoming.len() == 1 { "1 fattura ricevuta da registrare".into() } else { format!("{} fatture ricevute da registrare", incoming.len()) },
            desc: names.join(" · "),
            amount: Some(incoming.iter().map(|e| e.total).sum()),
            action_icon: icon::CHECK,
            action: "Registra",
            go: Go::Nav(Nav::Go(Screen::EInvoicesIn)),
        });
    }
    for d in drafts.iter().take(3) {
        todo.push(Todo {
            icon: icon::NOTE_PENCIL,
            bad: false,
            title: format!("{} in bozza", d.kind.label()),
            desc: format!("{} · creato il {}", if d.party_name.is_empty() { "senza cliente" } else { &d.party_name }, d.date.format("%d/%m/%Y")),
            amount: Some(d.total),
            action_icon: icon::PENCIL_SIMPLE,
            action: "Completa",
            go: Go::Nav(Nav::OpenDocument(d.id)),
        });
    }
    if let Some(r) = under_min.first() {
        let more = if under_min.len() > 1 { format!(" e altri {}", under_min.len() - 1) } else { String::new() };
        todo.push(Todo {
            icon: icon::PACKAGE,
            bad: false,
            title: format!("{} sotto scorta{}", r.product.description, more),
            desc: format!("{} {} in giacenza, scorta minima {}", openfatt_core::money::fmt_it_trim(r.stock, 0, 3), r.product.unit, openfatt_core::money::fmt_it_trim(r.product.min_stock, 0, 3)),
            amount: None,
            action_icon: icon::WAREHOUSE,
            action: "Vedi magazzino",
            go: Go::Nav(Nav::Go(Screen::Stock)),
        });
    }
    if let Some(inv) = &open_inv {
        let (t, _) = super::memo(&ctx, "home_inventory", gen, inv.id, || s.inventory_totals(inv.id).unwrap_or_default());
        todo.push(Todo {
            icon: icon::LIST_CHECKS,
            bad: false,
            title: format!("Inventario {} in corso", inv.warehouse),
            desc: format!("contati {} articoli su {}", t.counted, t.lines),
            amount: None,
            action_icon: icon::ARROW_RIGHT,
            action: "Continua",
            go: Go::Nav(Nav::Go(Screen::Inventory)),
        });
    }
    let (rec, _) = super::memo(&ctx, "home_recurrences", gen, today, || s.due_recurrences(today).unwrap_or_default());
    if !rec.is_empty() {
        todo.push(Todo {
            icon: icon::REPEAT,
            bad: false,
            title: if rec.len() == 1 { format!("Documento ricorrente da generare: {}", rec[0].description) } else { format!("{} documenti ricorrenti da generare", rec.len()) },
            desc: format!("previsto dal {}", rec.iter().map(|r| r.next_date).min().unwrap_or(today).format("%d/%m/%Y")),
            amount: None,
            action_icon: icon::REPEAT,
            action: "Vedi",
            go: Go::Nav(Nav::Go(Screen::Extra(super::business::Extra::Recurring))),
        });
    }
    let (agenda, _) = super::memo(&ctx, "home_agenda_week", gen, today, || s.agenda(today, today + chrono::Duration::days(7)).unwrap_or_default().into_iter().filter(|a| !a.done).collect::<Vec<_>>());
    let (late, _) = super::memo(&ctx, "home_agenda_late", gen, today, || s.agenda_overdue(today).unwrap_or_default());
    if !late.is_empty() {
        todo.push(Todo {
            icon: icon::CALENDAR_BLANK,
            bad: true,
            title: if late.len() == 1 { "1 attività arretrata in agenda".into() } else { format!("{} attività arretrate in agenda", late.len()) },
            desc: format!("la più vecchia del {}", late.first().map(|a| a.date.format("%d/%m/%Y").to_string()).unwrap_or_default()),
            amount: None,
            action_icon: icon::CALENDAR_BLANK,
            action: "Agenda",
            go: Go::Nav(Nav::Go(Screen::Extra(super::business::Extra::Agenda))),
        });
    }

    // fatturato: ultimi sei mesi e anno in corso
    let mut month_labels = Vec::new();
    let mut month_values = Vec::new();
    for i in 0..6 {
        let m0 = first_month.month0() as i32 + i;
        let (y, m) = (first_month.year() + m0 / 12, (m0 % 12) as u32);
        month_labels.push(MONTHS_SHORT[m as usize].to_string());
        let v: Decimal = recent_docs
            .iter()
            .filter(|d| d.status == DocStatus::Issued && d.date.year() == y && d.date.month0() == m)
            .filter_map(|d| sales_sign(d.kind).map(|k| k * d.total))
            .sum();
        month_values.push(v.to_f64().unwrap_or(0.0));
    }
    let year_total: Decimal = hd.year_docs.iter().filter_map(|d| sales_sign(d.kind).map(|k| k * d.total)).sum();

    Facts {
        today,
        todo,
        overdue_count: overdue.len(),
        oldest_overdue_days: overdue.first().map(|d| (today - d.date).num_days()).unwrap_or(0),
        receivables: receivables.clone(),
        open_sum,
        overdue_sum,
        in30,
        payables,
        payables_overdue,
        payables_count: payable_rows.len(),
        year_total,
        month_labels,
        month_values,
        rejected: rejected.len(),
        ready: ready.len(),
        sent,
        to_generate: without_fe.len(),
        to_register: incoming.len(),
        agenda: agenda.as_ref().clone(),
        agenda_late: late.len(),
        under_min: under_min.clone(),
        recent: recent_docs.iter().take(8).cloned().collect(),
    }
}

// ============================================================================ schermata

/// Cambi della disposizione chiesti durante il disegno, applicati alla fine.
enum Act {
    Move(usize, usize),
    Span(usize, u8),
    Accent(usize, Option<u8>),
    Hide(usize),
    Show(Card),
    Reset,
    AddShortcut(String),
    RemoveShortcut(usize),
    AddNote,
    RemoveNote(u64),
    Appearance,
}

pub fn show(ui: &mut Ui, cx: &mut Ctx, st: &mut State) {
    let s = cx.store;
    let today = chrono::Local::now().date_naive();
    let company = s.company().unwrap_or_default();

    if let Some((title, issues)) = &st.issues {
        let mut close = false;
        egui::Window::new(title.as_str()).collapsible(false).default_width(640.0).show(ui.ctx(), |ui| {
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for i in issues {
                    ui.label(i.to_string());
                }
            });
            if theme::button(ui, "", "Chiudi").clicked() {
                close = true;
            }
        });
        if close {
            st.issues = None;
        }
    }

    st.sync(s);
    let f = gather(ui, s, st, today);
    let mut acts: Vec<Act> = Vec::new();
    let mut go: Vec<Go> = Vec::new();

    // ---------------- barra del titolo ----------------
    let editing = st.editing;
    theme::title_row(ui, &[], "Start", Some(theme::subline(&[("Scrivania", false)])), |ui| {
        if editing {
            if theme::primary(ui, icon::CHECK, "Fine").clicked() {
                st.editing = false;
            }
            if theme::button(ui, icon::ARROW_COUNTER_CLOCKWISE, "Ripristina").on_hover_text("Torna alla disposizione iniziale delle schede").clicked() {
                acts.push(Act::Reset);
            }
        } else {
            if theme::primary(ui, icon::PLUS, "Nuova fattura").clicked() {
                cx.nav.push(Nav::NewDocument(DocKind::Invoice));
            }
            if theme::button(ui, icon::SLIDERS_HORIZONTAL, "Personalizza").on_hover_text("Sposta, ridimensiona, colora o nascondi le schede della scrivania").clicked() {
                st.editing = true;
            }
        }
    });

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(4.0);
        greeting(ui, &company, &f);
        ui.add_space(14.0);
        if editing {
            edit_banner(ui, st, &mut acts);
            ui.add_space(12.0);
        }
        board(ui, cx, st, &f, &mut acts, &mut go);
        ui.add_space(12.0);
    });

    for g in go {
        match g {
            Go::Nav(n) => cx.nav.push(n),
            Go::Generate(id) => {
                super::documents::generate_einvoices(cx, &[id], &mut st.issues);
                cx.nav.push(Nav::Go(Screen::EInvoicesOut));
            }
        }
    }
    if acts.iter().any(|a| matches!(a, Act::Appearance)) {
        cx.nav.push(Nav::Appearance);
    }
    for a in acts {
        apply(st, a, today);
    }
    let ctx = ui.ctx().clone();
    st.persist(&ctx, s, cx.msgs);
}

fn apply(st: &mut State, a: Act, today: NaiveDate) {
    match a {
        Act::Move(from, to) => {
            if from < st.cards.len() && to < st.cards.len() && from != to {
                let c = st.cards.remove(from);
                st.cards.insert(to, c);
            }
        }
        Act::Span(i, n) => {
            if let Some(c) = st.cards.get_mut(i) {
                c.span = n;
            }
        }
        Act::Accent(i, a) => {
            if let Some(c) = st.cards.get_mut(i) {
                c.accent = a;
            }
        }
        Act::Hide(i) => {
            if let Some(c) = st.cards.get_mut(i) {
                c.hidden = true;
            }
        }
        Act::Show(k) => {
            if let Some(p) = st.cards.iter().position(|c| c.kind == k) {
                let mut c = st.cards.remove(p);
                c.hidden = false;
                st.cards.push(c);
            }
        }
        Act::Reset => {
            st.cards = default_cards();
            st.shortcuts = DEFAULT_SHORTCUTS.iter().map(|s| s.to_string()).collect();
        }
        Act::AddShortcut(code) => {
            if !st.shortcuts.contains(&code) {
                st.shortcuts.push(code);
            }
        }
        Act::RemoveShortcut(i) => {
            if i < st.shortcuts.len() {
                st.shortcuts.remove(i);
            }
        }
        Act::AddNote => {
            let id = st.notes.iter().map(|n| n.id).max().unwrap_or(0) + 1;
            let color = st.notes.last().map(|n| (n.color + 1) % PAPERS.len() as u8).unwrap_or(0);
            st.notes.push(Note { id, text: String::new(), color, done: false, date: today });
            st.focus_note = Some(id);
        }
        Act::RemoveNote(id) => st.notes.retain(|n| n.id != id),
        Act::Appearance => {}
    }
}

fn greeting(ui: &mut Ui, company: &openfatt_core::model::Company, f: &Facts) {
    let hour = chrono::Local::now().hour();
    let hello = if hour < 13 { "Buongiorno" } else if hour < 18 { "Buon pomeriggio" } else { "Buonasera" };
    let name = if !company.name.is_empty() { company.name.clone() } else { format!("{} {}", company.first_name, company.last_name).trim().to_string() };
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(theme::bold(if name.is_empty() { hello.to_string() } else { format!("{}, {}", hello, name) }).size(24.0));
            let mut parts: Vec<(String, bool)> = vec![(long_date(f.today), false)];
            let urgent = f.todo.iter().filter(|t| t.bad).count();
            parts.push((" · ".into(), false));
            parts.push((
                if f.todo.is_empty() {
                    "nessuna attività in sospeso".into()
                } else if urgent > 0 {
                    format!("{} attività, {} urgenti", f.todo.len(), urgent)
                } else {
                    format!("{} attività da completare", f.todo.len())
                },
                urgent > 0,
            ));
            if f.overdue_sum > Decimal::ZERO {
                parts.push((" · ".into(), false));
                parts.push((format!("€ {} di crediti scaduti", eur(f.overdue_sum)), true));
            }
            ui.label(theme::subline(&parts.iter().map(|(t, b)| (t.as_str(), *b)).collect::<Vec<_>>()));
        });
    });
}

fn edit_banner(ui: &mut Ui, st: &State, acts: &mut Vec<Act>) {
    let t = theme::tint();
    egui::Frame::new().fill(t.tint).stroke(Stroke::new(1.0_f32, theme::mix(t.tint, t.main, 0.3))).corner_radius(10).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::SLIDERS_HORIZONTAL).size(18.0).color(t.main));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(theme::medium("Stai personalizzando la scrivania").color(t.dark));
                ui.label(RichText::new(format!(
                    "Trascina le schede dalla maniglia {} per spostarle · scegli larghezza e colore · {} per nasconderle · le scorciatoie si tolgono con ×",
                    icon::DOTS_SIX_VERTICAL,
                    icon::EYE_SLASH
                ))
                .size(12.5)
                .color(theme::INK_SOFT));
            });
            let hidden: Vec<Card> = st.cards.iter().filter(|c| c.hidden).map(|c| c.kind).collect();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::button(ui, icon::PALETTE, "Colori e barra delle sezioni...").on_hover_text("Colore di ogni sezione, ordine dei pulsanti della barra, finestre").clicked() {
                    acts.push(Act::Appearance);
                }
                ui.add_enabled_ui(!hidden.is_empty(), |ui| {
                    let b = theme::button_widget(icon::PLUS, &if hidden.is_empty() { "Tutte le schede sono visibili".to_string() } else { format!("Aggiungi scheda ({})", hidden.len()) });
                    egui::menu::menu_custom_button(ui, b, |ui| {
                        for k in &hidden {
                            if ui.button(format!("{}  {}", k.icon(), k.title())).clicked() {
                                acts.push(Act::Show(*k));
                                ui.close_menu();
                            }
                        }
                    });
                });
            });
        });
    });
}

// ---------------------------------------------------------------------------- griglia

const GAP: f32 = 14.0;

fn board(ui: &mut Ui, cx: &mut Ctx, st: &mut State, f: &Facts, acts: &mut Vec<Act>, go: &mut Vec<Go>) {
    let full_w = ui.available_width();
    let cols: usize = if full_w >= 1180.0 { 4 } else if full_w >= 680.0 { 2 } else { 1 };
    let col_w = (full_w - GAP * (cols as f32 - 1.0)) / cols as f32;
    let editing = st.editing;

    // righe: schede visibili nell'ordine, a capo quando non c'è più posto
    let mut rows: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut cur: Vec<(usize, usize)> = Vec::new();
    let mut used = 0;
    for (i, c) in st.cards.iter().enumerate().filter(|(_, c)| !c.hidden) {
        let span = (c.span as usize).clamp(1, cols);
        if used + span > cols {
            rows.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push((i, span));
        used += span;
    }
    if !cur.is_empty() {
        rows.push(cur);
    }

    let ctx = ui.ctx().clone();
    let dragging = egui::DragAndDrop::payload::<usize>(&ctx).map(|p| *p);
    for row in &rows {
        let top = ui.cursor().top();
        let left = ui.max_rect().left();
        let key = egui::Id::new(("home_row_h", row.iter().map(|(i, _)| st.cards[*i].kind).collect::<Vec<_>>(), cols));
        let target: f32 = ctx.data(|d| d.get_temp(key)).unwrap_or(0.0);
        let mut natural_max: f32 = 0.0;
        let mut bottom = top;
        let mut x = left;
        for &(i, span) in row {
            let w = col_w * span as f32 + GAP * (span as f32 - 1.0);
            let rect = egui::Rect::from_min_size(egui::pos2(x, top), Vec2::new(w, 4000.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(("home_card", st.cards[i].kind)));
            let (natural, frame_rect) = card(&mut child, cx, st, f, i, target, dragging, acts, go);
            natural_max = natural_max.max(natural);
            bottom = bottom.max(frame_rect.bottom());
            // in modalità Personalizza la scheda accoglie quella trascinata
            if editing {
                let r = child.interact(frame_rect, egui::Id::new(("home_drop", st.cards[i].kind)), Sense::hover());
                if let Some(from) = r.dnd_hover_payload::<usize>() {
                    if *from != i {
                        let bar_x = if *from < i { frame_rect.right() + GAP / 2.0 } else { frame_rect.left() - GAP / 2.0 };
                        ui.painter().rect_stroke(frame_rect, 12, Stroke::new(2.0_f32, theme::tint().main), egui::StrokeKind::Outside);
                        ui.painter().vline(bar_x, frame_rect.y_range(), Stroke::new(4.0_f32, theme::tint().main));
                    }
                }
                if let Some(from) = r.dnd_release_payload::<usize>() {
                    acts.push(Act::Move(*from, i));
                }
            }
            x += w + GAP;
        }
        if (natural_max - target).abs() > 0.5 {
            ctx.data_mut(|d| d.insert_temp(key, natural_max));
            ctx.request_repaint();
        }
        ui.advance_cursor_after_rect(egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(left + full_w, bottom)));
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }

    // etichetta che segue il mouse mentre si trascina una scheda
    if let (Some(from), Some(pos)) = (dragging, ctx.pointer_interact_pos()) {
        if let Some(c) = st.cards.get(from) {
            let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("home_drag")));
            let text = format!("{}  {}", c.kind.icon(), c.kind.title());
            let g = p.layout_no_wrap(text, egui::FontId::new(14.0, theme::medium_family()), Color32::WHITE);
            let r = egui::Rect::from_min_size(pos + Vec2::new(14.0, 10.0), g.size() + Vec2::new(24.0, 14.0));
            p.rect_filled(r.translate(Vec2::new(0.0, 3.0)), 10, Color32::from_black_alpha(40));
            p.rect_filled(r, 10, slot_accent(c));
            p.galley(r.min + Vec2::new(12.0, 7.0), g, Color32::WHITE);
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }
}

fn slot_accent(c: &Slot) -> Color32 {
    c.accent.map(accent_color).unwrap_or_else(|| c.kind.section().main())
}

/// Disegna una scheda; restituisce l'altezza naturale (senza il riempimento per pareggiare
/// la riga) e il rettangolo occupato.
#[allow(clippy::too_many_arguments)]
fn card(ui: &mut Ui, cx: &mut Ctx, st: &mut State, f: &Facts, i: usize, target: f32, dragging: Option<usize>, acts: &mut Vec<Act>, go: &mut Vec<Go>) -> (f32, egui::Rect) {
    let slot = st.cards[i].clone();
    let accent = slot_accent(&slot);
    let editing = st.editing;
    let soft = theme::mix(Color32::WHITE, accent, 0.13);
    let being_dragged = dragging == Some(i);
    let stroke = if editing { Stroke::new(1.0_f32, theme::mix(theme::BORDER, accent, 0.35)) } else { Stroke::new(1.0_f32, theme::BORDER) };
    let frame = egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(stroke)
        .corner_radius(12)
        .inner_margin(Margin { left: 16, right: 14, top: 12, bottom: 14 })
        .shadow(egui::epaint::Shadow { offset: [0, 1], blur: 6, spread: 0, color: Color32::from_black_alpha(10) });
    let pad_v = 12.0 + 14.0 + 2.0;
    let r = frame.show(ui, |ui| {
        if being_dragged {
            ui.multiply_opacity(0.35);
        }
        ui.set_width(ui.available_width());
        // intestazione
        ui.horizontal(|ui| {
            ui.set_min_height(28.0);
            if editing {
                let h = ui.add(egui::Label::new(RichText::new(icon::DOTS_SIX_VERTICAL).size(20.0).color(theme::MUTED)).sense(Sense::drag())).on_hover_text("Trascina per spostare la scheda").on_hover_cursor(egui::CursorIcon::Grab);
                h.dnd_set_drag_payload(i);
            }
            if !slot.kind.kpi() {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                ui.painter().rect_filled(r, 8, soft);
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, slot.kind.icon(), egui::FontId::proportional(16.0), accent);
                ui.label(theme::medium(slot.kind.title()).size(15.0));
            } else {
                ui.label(RichText::new(slot.kind.icon()).size(16.0).color(accent));
                ui.label(theme::medium(slot.kind.title()).size(13.5).color(theme::INK_SOFT));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if editing {
                    edit_controls(ui, &slot, i, accent, acts);
                } else {
                    header_right(ui, slot.kind, f, st, accent, acts, go);
                }
            });
        });
        ui.add_space(if slot.kind.kpi() { 2.0 } else { 8.0 });
        theme::with_section(slot.kind.section(), || body(ui, cx, st, f, &slot, accent, acts, go));
        let natural = ui.min_rect().height() + pad_v;
        // pareggia l'altezza con le altre schede della riga
        if target > natural + 0.5 {
            ui.add_space(target - natural);
        }
        natural
    });
    // filo colorato in alto per i riepiloghi
    if slot.kind.kpi() {
        let rr = r.response.rect;
        let strip = egui::Rect::from_min_size(rr.min + Vec2::new(16.0, 0.0), Vec2::new(36.0, 3.0));
        ui.painter().rect_filled(strip, CornerRadius { nw: 0, ne: 0, sw: 2, se: 2 }, accent);
    }
    (r.inner, r.response.rect)
}

fn edit_controls(ui: &mut Ui, slot: &Slot, i: usize, accent: Color32, acts: &mut Vec<Act>) {
    if theme::icon_button(ui, icon::EYE_SLASH, "Nascondi la scheda").clicked() {
        acts.push(Act::Hide(i));
    }
    // colore
    let id = ui.make_persistent_id(("home_color", slot.kind));
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
    ui.painter().circle_filled(r.center(), 9.0, accent);
    ui.painter().circle_stroke(r.center(), 11.5, Stroke::new(1.0_f32, if resp.hovered() { accent } else { theme::BORDER }));
    let resp = resp.on_hover_text("Colore della scheda").on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(id));
    }
    egui::popup_below_widget(ui, id, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
        ui.set_min_width(200.0);
        ui.label(theme::muted("Colore della scheda").size(12.0));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(4.0);
            for (n, a) in ACCENTS.iter().enumerate() {
                let c = Color32::from_rgb(a.0, a.1, a.2);
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
                ui.painter().circle_filled(r.center(), 10.0, c);
                if slot.accent == Some(n as u8) {
                    ui.painter().circle_stroke(r.center(), 12.5, Stroke::new(2.0_f32, c));
                }
                if resp.on_hover_text(a.3).clicked() {
                    acts.push(Act::Accent(i, Some(n as u8)));
                }
            }
        });
        if ui.button(format!("{}  Colore della sezione", icon::ARROW_COUNTER_CLOCKWISE)).clicked() {
            acts.push(Act::Accent(i, None));
        }
    });
    ui.add_space(4.0);
    // larghezza
    let spans = slot.kind.spans();
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for n in [4u8, 2, 1] {
            let allowed = spans.contains(&n);
            let on = slot.span == n;
            let (label, tip) = match n {
                1 => ("S", "Stretta: una colonna"),
                2 => ("M", "Media: due colonne"),
                _ => ("L", "Larga: tutta la riga"),
            };
            let (r, resp) = ui.allocate_exact_size(Vec2::new(28.0, 24.0), if allowed { Sense::click() } else { Sense::hover() });
            let bg = if on { accent } else if resp.hovered() && allowed { theme::mix(Color32::WHITE, accent, 0.12) } else { theme::RAISED };
            let cr = match n {
                4 => CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 },
                1 => CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 },
                _ => CornerRadius::ZERO,
            };
            ui.painter().rect(r, cr, bg, Stroke::new(1.0_f32, if on { accent } else { theme::BORDER }), egui::StrokeKind::Inside);
            let fg = if on { Color32::WHITE } else if allowed { theme::INK } else { theme::CONTROL };
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::new(12.0, theme::medium_family()), fg);
            if allowed {
                if resp.on_hover_text(tip).clicked() {
                    acts.push(Act::Span(i, n));
                }
            }
        }
    });
}

fn link(ui: &mut Ui, text: &str) -> bool {
    ui.add(egui::Label::new(RichText::new(format!("{}  {}", text, icon::ARROW_RIGHT)).size(13.0).color(theme::INK_SOFT)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn header_right(ui: &mut Ui, kind: Card, f: &Facts, st: &mut State, _accent: Color32, acts: &mut Vec<Act>, go: &mut Vec<Go>) {
    match kind {
        Card::Todo => {
            let n = f.todo.len();
            if n > TODO_ROWS {
                let t = if st.todo_all { "Mostra meno".to_string() } else { format!("Tutte ({})", n) };
                if ui.add(egui::Label::new(RichText::new(t).size(13.0).color(theme::INK_SOFT)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    st.todo_all = !st.todo_all;
                }
            } else {
                ui.label(RichText::new(if n == 1 { "1 attività".into() } else { format!("{} attività", n) }).size(13.0).color(MUTED));
            }
        }
        Card::Incoming => {
            if link(ui, "Scadenzario") {
                go.push(Go::Nav(Nav::Go(Screen::Dues)));
            }
        }
        Card::Recent => {
            if link(ui, "Tutte le fatture") {
                go.push(Go::Nav(Nav::Go(Screen::Documents(DocGroup::Invoices))));
            }
        }
        Card::Agenda => {
            if link(ui, "Agenda") {
                go.push(Go::Nav(Nav::Go(Screen::Extra(super::business::Extra::Agenda))));
            }
        }
        Card::Chart => {
            ui.label(RichText::new("fatture e banco, IVA inclusa").size(12.0).color(MUTED));
        }
        Card::Notes => {
            if theme::ghost(ui, true, icon::PLUS, "Nuovo").clicked() {
                acts.push(Act::AddNote);
            }
        }
        Card::Stock => {
            if link(ui, "Magazzino") {
                go.push(Go::Nav(Nav::Go(Screen::Stock)));
            }
        }
        Card::EInvoices => {
            if link(ui, "Inviate") {
                go.push(Go::Nav(Nav::Go(Screen::EInvoicesOut)));
            }
        }
        _ => {}
    }
}

const TODO_ROWS: usize = 6;

#[allow(clippy::too_many_arguments)]
fn body(ui: &mut Ui, _cx: &mut Ctx, st: &mut State, f: &Facts, slot: &Slot, accent: Color32, acts: &mut Vec<Act>, go: &mut Vec<Go>) {
    match slot.kind {
        Card::Receivable => {
            kpi(ui, eur(f.open_sum), theme::INK, &format!("€ {} entro 30 giorni", eur(f.in30)), go, Nav::Go(Screen::Dues));
            let total = (f.overdue_sum + f.in30).max(Decimal::ZERO);
            let rest = (f.open_sum - total).max(Decimal::ZERO);
            split_bar(ui, &[(f.overdue_sum, NEGATIVE, "scaduto"), (f.in30, accent, "entro 30 giorni"), (rest, theme::mix(Color32::WHITE, accent, 0.3), "più avanti")]);
        }
        Card::Overdue => {
            let sub = if f.overdue_count == 0 { "nessuna scadenza passata".to_string() } else { format!("{} scadenze · la più vecchia da {} giorni", f.overdue_count, f.oldest_overdue_days) };
            kpi(ui, eur(f.overdue_sum), if f.overdue_sum.is_zero() { theme::INK } else { NEGATIVE }, &sub, go, Nav::Reminders)
        }
        Card::Revenue => {
            let month = f.month_values.last().copied().unwrap_or(0.0);
            kpi(ui, eur(f.year_total), theme::INK, &format!("{}: € {}", MONTHS[f.today.month0() as usize], eur(Decimal::from_f64_retain(month).unwrap_or_default().round_dp(2))), go, Nav::Go(Screen::Extra(super::business::Extra::Stats)));
            sparkline(ui, &f.month_values, accent);
        }
        Card::Payables => {
            let sub = if f.payables_overdue > Decimal::ZERO { format!("di cui scaduti € {}", eur(f.payables_overdue)) } else { format!("{} scadenze aperte", f.payables_count) };
            kpi(ui, eur(f.payables), theme::INK, &sub, go, Nav::Payments(super::ledger::PayView::Out));
        }
        Card::Todo => {
            if f.todo.is_empty() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::CHECK_CIRCLE).size(20.0).color(theme::POSITIVE));
                    ui.label(theme::muted("Tutto in ordine: nessuna fattura da inviare, nessuno scarto, nessuna scadenza passata."));
                });
            }
            let n = if st.todo_all { f.todo.len() } else { TODO_ROWS };
            for (k, t) in f.todo.iter().take(n).enumerate() {
                if k > 0 {
                    theme::rule(ui);
                }
                if todo_row(ui, t) {
                    go.push(match &t.go {
                        Go::Nav(n) => Go::Nav(clone_nav(n)),
                        Go::Generate(id) => Go::Generate(*id),
                    });
                }
            }
        }
        Card::Shortcuts => shortcuts(ui, st, acts, go),
        Card::Notes => notes(ui, st, acts),
        Card::Chart => {
            theme::bar_chart(ui, &f.month_labels, &f.month_values, 180.0, |v| {
                if v >= 1000.0 {
                    eur(Decimal::from_f64_retain(v).unwrap_or_default().round()).trim_end_matches(",00").to_string()
                } else {
                    format!("{}", v.round())
                }
            });
        }
        Card::Incoming => {
            if f.receivables.is_empty() {
                ui.label(theme::muted("Nessuna scadenza aperta."));
            }
            for (k, d) in f.receivables.iter().take(5).enumerate() {
                if k > 0 {
                    theme::rule(ui);
                }
                let late = d.date < f.today;
                ui.horizontal(|ui| {
                    ui.set_min_height(40.0);
                    date_block(ui, d.date, late);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.set_width((ui.available_width() - 84.0).max(60.0));
                        match d.party_id {
                            Some(id) => {
                                super::quick::link(ui, truncate(&d.party_name, 30), super::quick::Ref::Party(id));
                            }
                            None => {
                                ui.label(truncate(&d.party_name, 30));
                            }
                        }
                        ui.add(egui::Label::new(RichText::new(format!("{}{}", short_doc(&d.doc_label), if late { " · scaduta" } else { "" })).size(12.0).color(if late { NEGATIVE } else { MUTED })).truncate());
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(theme::medium(eur(d.residual())));
                    });
                });
            }
        }
        Card::Agenda => {
            if f.agenda_late > 0 {
                ui.label(RichText::new(format!("{}  {} attività arretrate", icon::WARNING_CIRCLE, f.agenda_late)).size(13.0).color(NEGATIVE));
                ui.add_space(4.0);
            }
            if f.agenda.is_empty() {
                ui.label(theme::muted("Nessun impegno nei prossimi 7 giorni."));
            }
            for (k, a) in f.agenda.iter().take(5).enumerate() {
                if k > 0 {
                    theme::rule(ui);
                }
                ui.horizontal(|ui| {
                    ui.set_min_height(40.0);
                    date_block(ui, a.date, false);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.add(egui::Label::new(theme::medium(&a.title)).truncate());
                        let when = if a.date == f.today { "oggi".to_string() } else { WEEKDAYS[a.date.weekday().num_days_from_monday() as usize].to_lowercase() };
                        ui.label(RichText::new(if a.time.is_empty() { when } else { format!("{} alle {}", when, a.time) }).size(12.0).color(MUTED));
                    });
                });
            }
            ui.add_space(4.0);
            if theme::ghost(ui, true, icon::PLUS, "Nuovo impegno").clicked() {
                go.push(Go::Nav(Nav::Go(Screen::Extra(super::business::Extra::Agenda))));
            }
        }
        Card::EInvoices => {
            let rows: [(&str, usize, Tone, Nav); 5] = [
                ("Da generare", f.to_generate, Tone::Warning, Nav::Go(Screen::EInvoicesOut)),
                ("Pronte da inviare", f.ready, Tone::Warning, Nav::Go(Screen::EInvoicesOut)),
                ("In attesa di esito", f.sent, Tone::Neutral, Nav::Go(Screen::EInvoicesOut)),
                ("Scartate dallo SdI", f.rejected, Tone::Negative, Nav::Go(Screen::EInvoicesOut)),
                ("Ricevute da registrare", f.to_register, Tone::Positive, Nav::Go(Screen::EInvoicesIn)),
            ];
            for (k, (label, n, tone, nav)) in rows.into_iter().enumerate() {
                if k > 0 {
                    theme::rule(ui);
                }
                let resp = ui
                    .horizontal(|ui| {
                        ui.set_min_height(30.0);
                        ui.label(if n == 0 { theme::muted(label) } else { RichText::new(label).color(theme::INK) });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if n == 0 {
                                ui.label(theme::muted("0"));
                            } else {
                                theme::pill(ui, &n.to_string(), tone);
                            }
                        });
                    })
                    .response;
                if n > 0 && ui.interact(resp.rect, ui.id().with(("fe_row", k)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    go.push(Go::Nav(nav));
                }
            }
        }
        Card::Stock => {
            if f.under_min.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::CHECK_CIRCLE).size(18.0).color(theme::POSITIVE));
                    ui.label(theme::muted("Nessun articolo sotto la scorta minima."));
                });
            }
            for (k, r) in f.under_min.iter().take(5).enumerate() {
                if k > 0 {
                    theme::rule(ui);
                }
                ui.horizontal(|ui| {
                    ui.set_min_height(36.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.set_width((ui.available_width() - 70.0).max(60.0));
                        ui.add(egui::Label::new(theme::medium(&r.product.description)).truncate());
                        ui.label(RichText::new(format!("scorta minima {}", openfatt_core::money::fmt_it_trim(r.product.min_stock, 0, 3))).size(12.0).color(MUTED));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format!("{} {}", openfatt_core::money::fmt_it_trim(r.stock, 0, 3), r.product.unit)).color(theme::WARNING).family(theme::medium_family()));
                    });
                });
            }
        }
        Card::Recent => recent(ui, f, go),
    }
}

/// Riepilogo: numero grande, una riga di spiegazione, tutta la scheda porta alla schermata.
fn kpi(ui: &mut Ui, value: String, color: Color32, sub: &str, go: &mut Vec<Go>, nav: Nav) {
    let r = ui
        .vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(format!("€ {}", value)).size(27.0).family(theme::bold_family()).color(color));
            ui.add(egui::Label::new(RichText::new(sub).size(12.5).color(MUTED)).truncate());
        })
        .response;
    if ui.interact(r.rect, ui.id().with("kpi"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        go.push(Go::Nav(nav));
    }
}

/// Barra divisa in parti proporzionali con la legenda sotto.
fn split_bar(ui: &mut Ui, parts: &[(Decimal, Color32, &str)]) {
    let total: f64 = parts.iter().map(|p| p.0.to_f64().unwrap_or(0.0).max(0.0)).sum();
    if total <= 0.0 {
        return;
    }
    ui.add_space(8.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 8.0), Sense::hover());
    ui.painter().rect_filled(rect, 4, theme::TRACK);
    let mut x = rect.left();
    for (v, c, _) in parts {
        let w = (v.to_f64().unwrap_or(0.0).max(0.0) / total) as f32 * rect.width();
        if w > 0.5 {
            ui.painter().rect_filled(egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x + w, rect.bottom())), 0, *c);
        }
        x += w;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for (v, c, label) in parts {
            if v.is_zero() {
                continue;
            }
            ui.label(RichText::new(format!("{} {}", icon::CIRCLE, label)).size(11.5).color(*c));
        }
    });
}

fn sparkline(ui: &mut Ui, values: &[f64], accent: Color32) {
    ui.add_space(6.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::hover());
    let max = values.iter().cloned().fold(0.0_f64, f64::max).max(1.0);
    let n = values.len().max(1);
    let slot = rect.width() / n as f32;
    for (i, v) in values.iter().enumerate() {
        let h = ((*v / max) as f32 * rect.height()).max(2.0);
        let x = rect.left() + slot * i as f32;
        let r = egui::Rect::from_min_max(egui::pos2(x + slot * 0.18, rect.bottom() - h), egui::pos2(x + slot * 0.82, rect.bottom()));
        let c = if i + 1 == n { accent } else { theme::mix(Color32::WHITE, accent, 0.25) };
        ui.painter().rect_filled(r, CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 }, c);
    }
}

fn date_block(ui: &mut Ui, d: NaiveDate, late: bool) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(40.0, 40.0), Sense::hover());
    let (bg, fg) = if late { (Color32::from_rgb(0xff, 0xed, 0xea), NEGATIVE) } else { (theme::RAISED, theme::INK) };
    ui.painter().rect_filled(r, 8, bg);
    ui.painter().text(egui::pos2(r.center().x, r.top() + 14.0), egui::Align2::CENTER_CENTER, format!("{:02}", d.day()), egui::FontId::new(15.0, theme::medium_family()), fg);
    ui.painter().text(egui::pos2(r.center().x, r.top() + 29.0), egui::Align2::CENTER_CENTER, MONTHS_SHORT[d.month0() as usize], egui::FontId::proportional(11.0), if late { NEGATIVE } else { MUTED });
}

fn shortcuts(ui: &mut Ui, st: &mut State, acts: &mut Vec<Act>, go: &mut Vec<Go>) {
    let editing = st.editing;
    let w = ui.available_width();
    let gap = 10.0;
    let mut items: Vec<Option<(usize, &'static Shortcut)>> = st.shortcuts.iter().enumerate().filter_map(|(i, c)| shortcut(c).map(|s| Some((i, s)))).collect();
    if editing {
        items.push(None);
    }
    // tessere larghe almeno 118, al massimo 10 per riga, allargate fino a riempire la riga
    let fit = ((w + gap) / (118.0 + gap)).floor().max(2.0) as usize;
    // poche scorciatoie non diventano tessere enormi: almeno 6 posti per riga
    let per_row = fit.min(items.len().max(6)).min(10);
    let tile_w = (w - gap * (per_row as f32 - 1.0)) / per_row as f32;
    let tile = Vec2::new(tile_w, 82.0);
    for chunk in items.chunks(per_row) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for it in chunk {
                match it {
                    Some((i, sc)) => {
                        let (r, resp) = ui.allocate_exact_size(tile, Sense::click());
                        let t = sc.section.tint();
                        let hov = resp.hovered() && !editing;
                        ui.painter().rect(r, 10, if hov { t.tint } else { theme::RAISED }, Stroke::new(1.0_f32, if hov { theme::mix(t.tint, t.main, 0.35) } else { theme::RULE }), egui::StrokeKind::Inside);
                        let c = egui::pos2(r.center().x, r.top() + 28.0);
                        ui.painter().circle_filled(c, 17.0, t.soft);
                        ui.painter().text(c, egui::Align2::CENTER_CENTER, sc.icon, egui::FontId::proportional(19.0), t.main);
                        let g = ui.painter().layout(sc.label.to_string(), egui::FontId::new(12.5, theme::medium_family()), theme::INK, tile_w - 10.0);
                        ui.painter().galley(egui::pos2(r.center().x - g.size().x / 2.0, r.top() + 52.0), g, theme::INK);
                        if editing {
                            let x = egui::Rect::from_center_size(egui::pos2(r.right() - 12.0, r.top() + 12.0), Vec2::splat(20.0));
                            let xr = ui.interact(x, ui.id().with(("sc_x", i)), Sense::click());
                            ui.painter().circle_filled(x.center(), 9.0, if xr.hovered() { NEGATIVE } else { theme::CONTROL });
                            ui.painter().text(x.center(), egui::Align2::CENTER_CENTER, icon::X, egui::FontId::proportional(11.0), Color32::WHITE);
                            if xr.on_hover_text("Togli la scorciatoia").clicked() {
                                acts.push(Act::RemoveShortcut(*i));
                            }
                        } else if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            if let Some(n) = shortcut_nav(sc.code) {
                                go.push(Go::Nav(n));
                            }
                        }
                    }
                    None => {
                        let (r, resp) = ui.allocate_exact_size(tile, Sense::click());
                        let t = theme::tint();
                        dashed_rect(ui.painter(), r, if resp.hovered() { t.main } else { theme::CONTROL });
                        ui.painter().text(egui::pos2(r.center().x, r.top() + 30.0), egui::Align2::CENTER_CENTER, icon::PLUS, egui::FontId::proportional(20.0), t.main);
                        ui.painter().text(egui::pos2(r.center().x, r.top() + 58.0), egui::Align2::CENTER_CENTER, "Aggiungi", egui::FontId::new(12.5, theme::medium_family()), theme::INK_SOFT);
                        let id = ui.make_persistent_id("home_add_shortcut");
                        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                        if resp.clicked() {
                            ui.memory_mut(|m| m.toggle_popup(id));
                        }
                        egui::popup_below_widget(ui, id, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                            ui.set_min_width(230.0);
                            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                                for sc in SHORTCUTS.iter().filter(|s| !st.shortcuts.iter().any(|c| c == s.code)) {
                                    let lbl = egui::text::LayoutJob::simple_singleline(format!("{}  {}", sc.icon, sc.label), egui::FontId::proportional(14.0), theme::INK);
                                    if ui.add(egui::Button::new(lbl).frame(false)).clicked() {
                                        acts.push(Act::AddShortcut(sc.code.to_string()));
                                    }
                                }
                            });
                        });
                    }
                }
            }
        });
        ui.add_space(gap - ui.spacing().item_spacing.y);
    }
}

fn dashed_rect(p: &egui::Painter, r: egui::Rect, c: Color32) {
    let s = Stroke::new(1.5_f32, c);
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in pts.windows(2) {
        p.extend(egui::Shape::dashed_line(&[w[0], w[1]], s, 6.0, 4.0));
    }
}

fn notes(ui: &mut Ui, st: &mut State, acts: &mut Vec<Act>) {
    let w = ui.available_width();
    let gap = 12.0;
    let per_row = ((w + gap) / (176.0 + gap)).floor().max(1.0) as usize;
    let note_w = (w - gap * (per_row as f32 - 1.0)) / per_row as f32;
    let size = Vec2::new(note_w, 150.0);
    let focus = st.focus_note.take();
    let count = st.notes.len();
    let mut idx: Vec<Option<usize>> = (0..count).map(Some).collect();
    idx.push(None);
    for chunk in idx.chunks(per_row) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for it in chunk {
                match it {
                    Some(k) => note(ui, &mut st.notes[*k], size, focus, acts),
                    None => {
                        let (r, resp) = ui.allocate_exact_size(size, Sense::click());
                        let c = if resp.hovered() { paper(0) } else { Color32::TRANSPARENT };
                        ui.painter().rect_filled(r, 6, c);
                        dashed_rect(ui.painter(), r, if resp.hovered() { theme::mix(paper(0), Color32::BLACK, 0.35) } else { theme::CONTROL });
                        ui.painter().text(r.center() - Vec2::new(0.0, 10.0), egui::Align2::CENTER_CENTER, icon::NOTE_BLANK, egui::FontId::proportional(24.0), theme::MUTED);
                        ui.painter().text(r.center() + Vec2::new(0.0, 18.0), egui::Align2::CENTER_CENTER, "Nuovo post-it", egui::FontId::new(13.0, theme::medium_family()), theme::INK_SOFT);
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            acts.push(Act::AddNote);
                        }
                    }
                }
            }
        });
        ui.add_space(gap - ui.spacing().item_spacing.y);
    }
}

fn note(ui: &mut Ui, n: &mut Note, size: Vec2, focus: Option<u64>, acts: &mut Vec<Act>) {
    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
    let bg = paper(n.color);
    let edge = theme::mix(bg, Color32::BLACK, 0.14);
    let ink = theme::mix(edge, Color32::BLACK, 0.62);
    let p = ui.painter().clone();
    // ombra e foglio con l'angolo in basso a destra un po' sollevato
    p.rect_filled(r.translate(Vec2::new(1.0, 3.0)), 6, Color32::from_black_alpha(16));
    p.rect_filled(r, CornerRadius { nw: 4, ne: 4, sw: 4, se: 14 }, bg);
    p.rect_filled(egui::Rect::from_min_size(r.min, Vec2::new(r.width(), 22.0)), CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 }, theme::mix(bg, edge, 0.45));
    p.text(egui::pos2(r.left() + 12.0, r.top() + 11.0), egui::Align2::LEFT_CENTER, icon::PUSH_PIN, egui::FontId::proportional(13.0), ink);
    p.text(egui::pos2(r.left() + 28.0, r.top() + 11.0), egui::Align2::LEFT_CENTER, format!("{} {}", n.date.day(), MONTHS_SHORT[n.date.month0() as usize]), egui::FontId::proportional(11.5), ink);
    let hovered = ui.rect_contains_pointer(r);
    // azioni: fatto, colore, elimina
    let mut x = r.right() - 14.0;
    for (ic, tip, what) in [(icon::X, "Elimina", 0), (icon::PALETTE, "Cambia colore", 1), (if n.done { icon::CHECK_SQUARE } else { icon::SQUARE }, "Fatto", 2)] {
        let br = egui::Rect::from_center_size(egui::pos2(x, r.top() + 11.0), Vec2::splat(18.0));
        let resp = ui.interact(br, ui.id().with(("note_btn", n.id, what)), Sense::click());
        if hovered || (what == 2 && n.done) {
            let c = if resp.hovered() { theme::INK } else { ink };
            p.text(br.center(), egui::Align2::CENTER_CENTER, ic, egui::FontId::proportional(13.0), c);
        }
        let resp = resp.on_hover_text(tip);
        if resp.clicked() {
            match what {
                0 => acts.push(Act::RemoveNote(n.id)),
                1 => n.color = (n.color + 1) % PAPERS.len() as u8,
                _ => n.done = !n.done,
            }
        }
        x -= 20.0;
    }
    let text_rect = egui::Rect::from_min_max(egui::pos2(r.left() + 10.0, r.top() + 28.0), egui::pos2(r.right() - 10.0, r.bottom() - 8.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(("note_text", n.id)));
    if n.done {
        // promemoria fatto: testo barrato, non modificabile finché non si toglie la spunta
        child.multiply_opacity(0.6);
        child.add(egui::Label::new(RichText::new(&n.text).font(egui::FontId::new(14.0, theme::medium_family())).color(ink).strikethrough()).wrap());
        return;
    }
    let te = egui::TextEdit::multiline(&mut n.text)
        .frame(false)
        .desired_width(text_rect.width())
        .desired_rows(5)
        .font(egui::FontId::new(14.0, theme::medium_family()))
        .text_color(ink)
        .hint_text(RichText::new("Scrivi un promemoria...").color(theme::mix(bg, ink, 0.5)))
        .margin(Margin::ZERO);
    let out = child.add(te);
    if focus == Some(n.id) {
        out.request_focus();
    }
}

fn recent(ui: &mut Ui, f: &Facts, go: &mut Vec<Go>) {
    if f.recent.is_empty() {
        ui.label(theme::muted("Nessun documento negli ultimi sei mesi."));
        return;
    }
    egui::Grid::new("recent").num_columns(5).spacing([24.0, 0.0]).min_row_height(34.0).show(ui, |ui| {
        for h in ["Documento", "Data", "Cliente / fornitore", "Totale", "Stato"] {
            ui.label(RichText::new(h).size(12.0).color(MUTED));
        }
        ui.end_row();
        for d in f.recent.iter().take(6) {
            if ui.add(egui::Label::new(theme::medium(format!("{} {}", d.kind.label(), d.number_label))).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                go.push(Go::Nav(Nav::OpenDocument(d.id)));
            }
            ui.label(d.date.format("%d/%m/%Y").to_string());
            match d.party_id {
                Some(id) if !d.party_name.is_empty() => {
                    super::quick::link(ui, truncate(&d.party_name, 34), super::quick::Ref::Party(id));
                }
                _ => {
                    ui.label(if d.party_name.is_empty() { "-".to_string() } else { truncate(&d.party_name, 34) });
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(eur(d.total));
            });
            let tone = match d.status {
                DocStatus::Draft => Tone::Warning,
                DocStatus::Issued => Tone::Neutral,
                DocStatus::Cancelled => Tone::Negative,
            };
            theme::pill(ui, d.status.label(), tone);
            ui.end_row();
        }
    });
}

/// Sezione a cui porta un'attività da fare (per il colore dell'icona).
fn todo_section(ic: &str) -> Section {
    match ic {
        x if x == icon::PAPER_PLANE_TILT || x == icon::TRAY_ARROW_DOWN || x == icon::ARROW_CLOCKWISE => Section::EInvoice,
        x if x == icon::ENVELOPE_SIMPLE || x == icon::BANK || x == icon::ARROW_SQUARE_UP_RIGHT || x == icon::CLOCK_COUNTDOWN => Section::Payments,
        x if x == icon::PERCENT => Section::Tools,
        x if x == icon::PACKAGE || x == icon::LIST_CHECKS => Section::Stock,
        x if x == icon::CALENDAR_BLANK => Section::Agenda,
        x if x == icon::BUILDINGS || x == icon::GEAR_SIX => Section::Options,
        _ => Section::Sales,
    }
}

fn todo_row(ui: &mut Ui, t: &Todo) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.set_min_height(48.0);
        let tn = todo_section(t.icon).tint();
        let (fg, bg) = if t.bad { (NEGATIVE, Color32::from_rgb(0xff, 0xed, 0xea)) } else { (tn.main, tn.soft) };
        let (r, _) = ui.allocate_exact_size(Vec2::new(30.0, 30.0), Sense::hover());
        ui.painter().rect_filled(r, 8, bg);
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, t.icon, egui::FontId::proportional(16.0), fg);
        let w = ui.available_width();
        let narrow = w < 470.0;
        ui.vertical(|ui| {
            ui.set_width((w - if narrow { 130.0 } else { 270.0 }).max(120.0));
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.add(egui::Label::new(theme::medium(&t.title)).truncate());
            ui.add(egui::Label::new(RichText::new(&t.desc).size(12.5).color(MUTED)).truncate());
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = if narrow { theme::button_widget(t.action_icon, "").min_size(Vec2::new(34.0, 30.0)) } else { theme::button_widget(t.action_icon, t.action).min_size(Vec2::new(150.0, 30.0)) };
            if ui.add(b).on_hover_text(t.action).clicked() {
                clicked = true;
            }
            ui.add_space(6.0);
            match t.amount {
                Some(a) => ui.label(theme::medium(eur(a))),
                None => ui.label(RichText::new("-").color(MUTED)),
            };
        });
    });
    clicked
}

fn short_doc(label: &str) -> String {
    truncate(label, 40)
}

fn first_line(s: &str) -> String {
    truncate(s.lines().next().unwrap_or(""), 80)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

fn clone_nav(n: &Nav) -> Nav {
    match n {
        Nav::Go(s) => Nav::Go(*s),
        Nav::OpenDocument(id) => Nav::OpenDocument(*id),
        Nav::PecSend(id) => Nav::PecSend(*id),
        Nav::Reminders => Nav::Reminders,
        Nav::BankTab(t) => Nav::BankTab(*t),
        Nav::VatTab(t) => Nav::VatTab(*t),
        Nav::PayDue(id) => Nav::PayDue(*id),
        _ => Nav::Go(Screen::Home),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposizione_predefinita_salvata_vuota() {
        let st = State::default();
        let p = st.prefs();
        assert!(p.cards.is_empty() && p.shortcuts.is_none() && p.notes.is_empty());
    }

    #[test]
    fn disposizione_salvata_e_riletta() {
        let mut st = State::default();
        st.cards.swap(0, 3);
        st.cards[1].span = 2;
        st.cards[2].accent = Some(4);
        st.cards[5].hidden = true;
        st.shortcuts = vec!["solleciti".into(), "nuova_fattura".into()];
        st.notes.push(Note { id: 7, text: "Chiamare il commercialista".into(), color: 2, done: true, date: NaiveDate::from_ymd_opt(2026, 11, 25).unwrap() });
        let json = serde_json::to_string(&st.prefs()).unwrap();
        let mut back = State::default();
        back.set_prefs(&serde_json::from_str(&json).unwrap());
        assert_eq!(back.cards, st.cards);
        assert_eq!(back.shortcuts, st.shortcuts);
        assert_eq!(back.notes, st.notes);
    }

    #[test]
    fn disposizione_di_altre_versioni() {
        // codici sconosciuti e larghezze non ammesse si scartano, le schede mancanti tornano in fondo
        let json = r#"{"cards":[{"code":"post_it","span":4},{"code":"scheda_futura","span":1},{"code":"da_fare","span":1,"accent":99}],"shortcuts":["clienti","funzione_futura"]}"#;
        let mut st = State::default();
        st.set_prefs(&serde_json::from_str(json).unwrap());
        assert_eq!(st.cards[0].kind, Card::Notes);
        assert_eq!(st.cards[0].span, 4);
        assert_eq!(st.cards[1].kind, Card::Todo);
        assert_eq!(st.cards[1].span, 2, "da fare non può stare in una colonna");
        assert_eq!(st.cards[1].accent, None);
        assert_eq!(st.cards.len(), Card::ALL.len());
        assert_eq!(st.shortcuts, vec!["clienti".to_string()]);
    }
}
