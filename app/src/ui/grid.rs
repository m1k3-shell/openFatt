//! Elenco riutilizzabile: colonne da mostrare, nascondere e spostare, ordinamento cliccando
//! l'intestazione, raggruppamento con subtotali, totali, selezione multipla (Ctrl e Maiusc),
//! esportazione in Excel di ciò che si vede. Le scelte sono salvate nell'archivio per ogni elenco.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;
use eframe::egui::{self, Ui};
use egui_extras::{Column, TableBuilder};
use openfatt_interop::table::{XColumn, XValue};
use openfatt_store::Store;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::theme::{self, icon};
use super::widgets::eur;
use super::Msgs;

/// Valore di una cella: serve per ordinare, raggruppare, sommare ed esportare.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Text(String),
    /// Numero con i decimali da mostrare.
    Num(Decimal, u32),
    Money(Decimal),
    Date(Option<NaiveDate>),
    Bool(bool),
}

impl Val {
    pub fn text(&self) -> String {
        match self {
            Val::Text(s) => s.clone(),
            Val::Num(d, dp) => openfatt_core::money::fmt_it_trim(*d, 0, *dp),
            Val::Money(d) => eur(*d),
            Val::Date(Some(d)) => d.format("%d/%m/%Y").to_string(),
            Val::Date(None) => String::new(),
            Val::Bool(b) => if *b { "sì".into() } else { String::new() },
        }
    }
    fn number(&self) -> Option<Decimal> {
        match self {
            Val::Num(d, _) | Val::Money(d) => Some(*d),
            _ => None,
        }
    }
    fn compare(&self, o: &Val) -> Ordering {
        match (self, o) {
            (Val::Num(a, _) | Val::Money(a), Val::Num(b, _) | Val::Money(b)) => a.cmp(b),
            (Val::Date(a), Val::Date(b)) => a.cmp(b),
            (Val::Bool(a), Val::Bool(b)) => a.cmp(b),
            _ => self.text().to_lowercase().cmp(&o.text().to_lowercase()),
        }
    }
    fn excel(&self) -> XValue {
        match self {
            Val::Text(s) if s.is_empty() => XValue::Empty,
            Val::Text(s) => XValue::Text(s.clone()),
            Val::Num(d, dp) => XValue::Number(d.to_f64().unwrap_or(0.0), *dp),
            Val::Money(d) => XValue::Money(d.to_f64().unwrap_or(0.0)),
            Val::Date(Some(d)) => XValue::Date(*d),
            Val::Date(None) => XValue::Empty,
            Val::Bool(b) => XValue::Bool(*b),
        }
    }
}

type ValueFn<'a, T> = Box<dyn Fn(&T) -> Val + 'a>;
type RenderFn<'a, T> = Box<dyn Fn(&mut Ui, &T) + 'a>;
type OpensFn<'a, T> = Box<dyn Fn(&T) -> Option<super::quick::Ref> + 'a>;

pub struct Col<'a, T> {
    pub id: &'static str,
    pub title: &'static str,
    pub width: f32,
    pub right: bool,
    pub shown: bool,
    pub total: bool,
    pub group: bool,
    pub value: ValueFn<'a, T>,
    pub render: Option<RenderFn<'a, T>>,
    /// Cliccando il testo si apre la scheda di questo elemento.
    pub opens: Option<OpensFn<'a, T>>,
}

impl<'a, T> Col<'a, T> {
    pub fn new(id: &'static str, title: &'static str, width: f32, value: impl Fn(&T) -> Val + 'a) -> Self {
        Col { id, title, width, right: false, shown: true, total: false, group: false, value: Box::new(value), render: None, opens: None }
    }
    /// Testo semplice.
    pub fn text(id: &'static str, title: &'static str, width: f32, f: impl Fn(&T) -> String + 'a) -> Self {
        Self::new(id, title, width, move |t| Val::Text(f(t)))
    }
    pub fn money(id: &'static str, title: &'static str, width: f32, f: impl Fn(&T) -> Decimal + 'a) -> Self {
        let mut c = Self::new(id, title, width, move |t| Val::Money(f(t)));
        c.right = true;
        c
    }
    /// Nascosta finché l'utente non la mostra.
    pub fn hidden(mut self) -> Self {
        self.shown = false;
        self
    }
    /// Sommata nei gruppi e nei totali.
    pub fn total(mut self) -> Self {
        self.total = true;
        self
    }
    /// Si può raggruppare per questa colonna.
    pub fn group(mut self) -> Self {
        self.group = true;
        self
    }
    pub fn right(mut self) -> Self {
        self.right = true;
        self
    }
    pub fn render(mut self, f: impl Fn(&mut Ui, &T) + 'a) -> Self {
        self.render = Some(Box::new(f));
        self
    }
    /// Il testo della cella apre la scheda del cliente, fornitore o articolo.
    pub fn opens(mut self, f: impl Fn(&T) -> Option<super::quick::Ref> + 'a) -> Self {
        self.opens = Some(Box::new(f));
        self
    }
}

/// Scelte dell'utente per un elenco (salvate nell'archivio).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Prefs {
    pub order: Vec<String>,
    pub hidden: Vec<String>,
    pub shown: Vec<String>,
    pub sort: Option<(String, bool)>,
    pub group: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum TextOp {
    #[default]
    Contains,
    NotContains,
    Equals,
    StartsWith,
    IsEmpty,
    NotEmpty,
}

impl TextOp {
    const ALL: [TextOp; 6] = [TextOp::Contains, TextOp::NotContains, TextOp::Equals, TextOp::StartsWith, TextOp::IsEmpty, TextOp::NotEmpty];
    fn label(self) -> &'static str {
        match self {
            TextOp::Contains => "contiene",
            TextOp::NotContains => "non contiene",
            TextOp::Equals => "è uguale a",
            TextOp::StartsWith => "inizia con",
            TextOp::IsEmpty => "è vuoto",
            TextOp::NotEmpty => "non è vuoto",
        }
    }
    fn needs_text(self) -> bool {
        !matches!(self, TextOp::IsEmpty | TextOp::NotEmpty)
    }
}

/// Condizione su una colonna: si usano i campi adatti al tipo di dato della colonna.
#[derive(Debug, Clone, PartialEq, Default)]
struct Filter {
    col: String,
    op: TextOp,
    text: String,
    min: Option<Decimal>,
    max: Option<Decimal>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    flag: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Number,
    Date,
    Bool,
}

impl Filter {
    fn is_set(&self) -> bool {
        (self.op != TextOp::Contains && !self.op.needs_text()) || !self.text.trim().is_empty() || self.min.is_some() || self.max.is_some() || self.from.is_some() || self.to.is_some() || self.flag.is_some()
    }

    fn matches(&self, v: &Val) -> bool {
        if !self.is_set() {
            return true;
        }
        match v {
            Val::Date(d) => match d {
                Some(d) => self.from.is_none_or(|f| *d >= f) && self.to.is_none_or(|t| *d <= t),
                None => self.from.is_none() && self.to.is_none(),
            },
            Val::Num(d, _) | Val::Money(d) => self.min.is_none_or(|m| *d >= m) && self.max.is_none_or(|m| *d <= m) && self.text_matches(&v.text()),
            Val::Bool(b) => self.flag.is_none_or(|f| f == *b),
            Val::Text(_) if self.min.is_some() || self.max.is_some() || self.from.is_some() || self.to.is_some() => false,
            Val::Text(_) => self.text_matches(&v.text()),
        }
    }

    fn text_matches(&self, value: &str) -> bool {
        let v = value.trim().to_lowercase();
        let q = self.text.trim().to_lowercase();
        match self.op {
            TextOp::IsEmpty => v.is_empty(),
            TextOp::NotEmpty => !v.is_empty(),
            _ if q.is_empty() => true,
            TextOp::Contains => v.contains(&q),
            TextOp::NotContains => !v.contains(&q),
            TextOp::Equals => v == q,
            TextOp::StartsWith => v.starts_with(&q),
        }
    }

    fn summary(&self) -> String {
        let d = |x: Option<NaiveDate>| x.map(|v| v.format("%d/%m/%Y").to_string()).unwrap_or_else(|| "...".into());
        if self.from.is_some() || self.to.is_some() {
            format!("{} - {}", d(self.from), d(self.to))
        } else if self.min.is_some() || self.max.is_some() {
            let n = |x: Option<Decimal>| x.map(|v| openfatt_core::money::fmt_it_trim(v, 0, 4)).unwrap_or_else(|| "...".into());
            format!("da {} a {}", n(self.min), n(self.max))
        } else if let Some(f) = self.flag {
            if f { "sì".into() } else { "no".into() }
        } else if self.op.needs_text() {
            format!("{} \"{}\"", self.op.label(), self.text.trim())
        } else {
            self.op.label().to_string()
        }
    }
}

/// Periodi pronti per i filtri sulle date.
fn periods(today: NaiveDate) -> Vec<(&'static str, NaiveDate, NaiveDate)> {
    use chrono::{Datelike, Duration};
    let ymd = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap_or(today);
    let month_end = |y: i32, m: u32| if m == 12 { ymd(y, 12, 31) } else { ymd(y, m + 1, 1) - Duration::days(1) };
    let (y, m) = (today.year(), today.month());
    let (py, pm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
    let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let q = (m - 1) / 3;
    vec![
        ("Oggi", today, today),
        ("Ieri", today - Duration::days(1), today - Duration::days(1)),
        ("Questa settimana", monday, monday + Duration::days(6)),
        ("Settimana scorsa", monday - Duration::days(7), monday - Duration::days(1)),
        ("Questo mese", ymd(y, m, 1), month_end(y, m)),
        ("Mese scorso", ymd(py, pm, 1), month_end(py, pm)),
        ("Questo trimestre", ymd(y, q * 3 + 1, 1), month_end(y, q * 3 + 3)),
        ("Quest'anno", ymd(y, 1, 1), ymd(y, 12, 31)),
        ("Anno scorso", ymd(y - 1, 1, 1), ymd(y - 1, 12, 31)),
        ("Ultimi 7 giorni", today - Duration::days(6), today),
        ("Ultimi 30 giorni", today - Duration::days(29), today),
        ("Ultimi 90 giorni", today - Duration::days(89), today),
        ("Ultimi 12 mesi", today - Duration::days(364), today),
    ]
}

#[derive(Clone)]
enum Line {
    Group { label: String, count: usize, sums: Vec<Option<Decimal>> },
    Item(usize),
}

#[derive(Default)]
pub struct Grid {
    pub name: &'static str,
    prefs: Prefs,
    loaded: bool,
    collapsed: BTreeSet<String>,
    pub selected: BTreeSet<i64>,
    anchor: Option<i64>,
    /// Versione dei dati passati dalla schermata: finché non cambia (con le stesse scelte di
    /// ordinamento e gruppi) ordine, gruppi e totali non si ricalcolano. 0 = sempre.
    pub version: u64,
    cache: Option<Cached>,
    filters: Vec<Filter>,
    /// Testo cercato in tutte le colonne, anche in quelle nascoste.
    search_all: String,
    filters_open: bool,
    shown: usize,
    /// Il clic su una riga la rende corrente senza toccare le righe spuntate.
    pub keep_selection: bool,
}

struct Cached {
    key: String,
    order: Vec<usize>,
    lines: Vec<Line>,
    keys: Vec<i64>,
    totals: Vec<(usize, Decimal)>,
}

#[derive(Default)]
pub struct Events {
    /// Doppio clic (o Invio) su una riga.
    pub opened: Option<i64>,
}

impl Grid {
    pub fn new(name: &'static str) -> Self {
        Grid { name, ..Default::default() }
    }

    fn load(&mut self, store: &Store) {
        if !self.loaded {
            let all: BTreeMap<String, Prefs> = store.get_setting("lists").unwrap_or_default();
            self.prefs = all.get(self.name).cloned().unwrap_or_default();
            self.loaded = true;
        }
    }

    fn save(&self, store: &Store) {
        let mut all: BTreeMap<String, Prefs> = store.get_setting("lists").unwrap_or_default();
        if self.prefs == Prefs::default() {
            all.remove(self.name);
        } else {
            all.insert(self.name.to_string(), self.prefs.clone());
        }
        let _ = store.set_setting("lists", &all);
    }

    /// Colonne nell'ordine scelto (indici in `cols`).
    fn ordered<T>(&self, cols: &[Col<T>]) -> Vec<usize> {
        let mut out: Vec<usize> = self.prefs.order.iter().filter_map(|id| cols.iter().position(|c| c.id == id)).collect();
        for i in 0..cols.len() {
            if !out.contains(&i) {
                out.push(i);
            }
        }
        out
    }

    fn visible<T>(&self, c: &Col<T>) -> bool {
        if self.prefs.hidden.iter().any(|h| h == c.id) {
            false
        } else {
            c.shown || self.prefs.shown.iter().any(|s| s == c.id)
        }
    }

    fn set_visible<T>(&mut self, c: &Col<T>, v: bool) {
        self.prefs.hidden.retain(|h| h != c.id);
        self.prefs.shown.retain(|h| h != c.id);
        if v && !c.shown {
            self.prefs.shown.push(c.id.into());
        }
        if !v && c.shown {
            self.prefs.hidden.push(c.id.into());
        }
    }

    fn filtering(&self) -> bool {
        !self.search_all.trim().is_empty() || self.filters.iter().any(|f| f.is_set())
    }

    /// La riga rispetta i filtri in vigore.
    pub fn accepts<T>(&self, cols: &[Col<T>], row: &T) -> bool {
        !self.filtering() || self.passes(cols, row)
    }

    /// Ultima riga cliccata.
    pub fn anchor(&self) -> Option<i64> {
        self.anchor
    }

    /// C'è una condizione attiva su questa colonna.
    pub fn has_filter(&self, col: &str) -> bool {
        self.filters.iter().any(|f| f.col == col && f.is_set())
    }

    fn passes<T>(&self, cols: &[Col<T>], row: &T) -> bool {
        let all_ok = self.filters.iter().all(|f| cols.iter().find(|c| c.id == f.col).is_none_or(|c| f.matches(&(c.value)(row))));
        if !all_ok {
            return false;
        }
        let q = self.search_all.trim().to_lowercase();
        q.is_empty() || q.split_whitespace().all(|w| cols.iter().any(|c| (c.value)(row).text().to_lowercase().contains(w)))
    }

    /// Tipo di dato di una colonna, ricavato dai valori.
    fn kind_of<T>(col: &Col<T>, rows: &[T]) -> Kind {
        let mut kind = None;
        for r in rows.iter().take(300) {
            match (col.value)(r) {
                Val::Date(_) => return Kind::Date,
                Val::Num(..) | Val::Money(_) => return Kind::Number,
                Val::Bool(_) => return Kind::Bool,
                Val::Text(s) if !s.is_empty() => kind = Some(Kind::Text),
                _ => {}
            }
        }
        kind.unwrap_or(Kind::Text)
    }

    /// Finestra dei filtri: una condizione per riga, su qualunque colonna, anche nascosta.
    fn filter_window<T>(&mut self, ctx: &egui::Context, cols: &[Col<T>], rows: &[T], title: &str) {
        let mut open = self.filters_open;
        let today = chrono::Local::now().date_naive();
        let mut remove = None;
        let mut close = false;
        egui::Window::new(format!("Filtri: {}", title)).id(egui::Id::new(("grid_filters", self.name))).open(&mut open).default_width(660.0).collapsible(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Cerca in tutte le colonne");
                ui.add(egui::TextEdit::singleline(&mut self.search_all).desired_width(320.0).hint_text("una o più parole"));
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for (n, f) in self.filters.iter_mut().enumerate() {
                    ui.push_id(n, |ui| {
                        ui.horizontal(|ui| {
                            let shown = cols.iter().find(|c| c.id == f.col).map(|c| c.title).unwrap_or("colonna...");
                            let before = f.col.clone();
                            egui::ComboBox::from_id_salt("col").width(150.0).height(340.0).selected_text(shown).show_ui(ui, |ui| {
                                let mut sorted: Vec<&Col<T>> = cols.iter().filter(|c| !c.title.is_empty()).collect();
                                sorted.sort_by_key(|c| c.title.to_lowercase());
                                for c in sorted {
                                    ui.selectable_value(&mut f.col, c.id.to_string(), c.title);
                                }
                            });
                            if before != f.col {
                                *f = Filter { col: f.col.clone(), ..Default::default() };
                            }
                            let Some(c) = cols.iter().find(|c| c.id == f.col) else { return };
                            match Self::kind_of(c, rows) {
                                Kind::Date => {
                                    egui::ComboBox::from_id_salt("period").width(130.0).height(340.0).selected_text("Periodo...").show_ui(ui, |ui| {
                                        for (name, a, b) in periods(today) {
                                            if ui.selectable_label(false, name).clicked() {
                                                f.from = Some(a);
                                                f.to = Some(b);
                                            }
                                        }
                                    });
                                    ui.label("dal");
                                    super::widgets::opt_date_field(ui, "from", &mut f.from);
                                    ui.label("al");
                                    super::widgets::opt_date_field(ui, "to", &mut f.to);
                                }
                                Kind::Number => {
                                    let field = |ui: &mut Ui, label: &str, salt: &str, v: &mut Option<Decimal>| {
                                        ui.label(label);
                                        let id = ui.make_persistent_id(salt);
                                        let mut s: String = ui.data_mut(|d| d.get_temp(id)).unwrap_or_else(|| v.map(|x| openfatt_core::money::fmt_it_trim(x, 0, 4)).unwrap_or_default());
                                        if ui.add(egui::TextEdit::singleline(&mut s).desired_width(90.0).horizontal_align(egui::Align::RIGHT)).changed() {
                                            *v = openfatt_core::money::parse_dec(&s);
                                        }
                                        ui.data_mut(|d| d.insert_temp(id, s));
                                    };
                                    field(ui, "da", "min", &mut f.min);
                                    field(ui, "a", "max", &mut f.max);
                                }
                                Kind::Bool => {
                                    ui.radio_value(&mut f.flag, Some(true), "Sì");
                                    ui.radio_value(&mut f.flag, Some(false), "No");
                                }
                                Kind::Text => {
                                    egui::ComboBox::from_id_salt("op").width(100.0).selected_text(f.op.label()).show_ui(ui, |ui| {
                                        for o in TextOp::ALL {
                                            ui.selectable_value(&mut f.op, o, o.label());
                                        }
                                    });
                                    if f.op.needs_text() {
                                        ui.add(egui::TextEdit::singleline(&mut f.text).desired_width(190.0));
                                        // valori presenti, se sono pochi: si sceglie invece di scrivere
                                        if rows.len() <= 50_000 {
                                            let mut values: BTreeSet<String> = BTreeSet::new();
                                            for r in rows {
                                                let t = (c.value)(r).text();
                                                if !t.trim().is_empty() {
                                                    values.insert(t);
                                                }
                                                if values.len() > 60 {
                                                    break;
                                                }
                                            }
                                            if values.len() <= 60 {
                                                ui.menu_button(icon::CARET_DOWN, |ui| {
                                                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                                                        for v in values {
                                                            if ui.button(&v).clicked() {
                                                                f.text = v.clone();
                                                                f.op = TextOp::Equals;
                                                                ui.close_menu();
                                                            }
                                                        }
                                                    });
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button(icon::X).on_hover_text("Togli questa condizione").clicked() {
                                    remove = Some(n);
                                }
                            });
                        });
                    });
                }
            });
            if let Some(n) = remove {
                self.filters.remove(n);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::button(ui, icon::PLUS, "Aggiungi condizione").clicked() {
                    let col = cols.iter().find(|c| !c.title.is_empty()).map(|c| c.id.to_string()).unwrap_or_default();
                    self.filters.push(Filter { col, ..Default::default() });
                }
                if theme::button(ui, icon::ARROW_COUNTER_CLOCKWISE, "Azzera").clicked() {
                    self.filters.clear();
                    self.search_all.clear();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::primary(ui, icon::CHECK, "Chiudi").clicked() {
                        close = true;
                    }
                    let n = rows.iter().filter(|r| self.passes(cols, *r)).count();
                    ui.label(theme::muted(format!("{} di {} righe", n, rows.len())));
                });
            });
        });
        self.filters_open = open && !close;
    }

    /// Condizioni attive sopra l'elenco, ciascuna si toglie con un clic.
    fn active_filters<T>(&mut self, ui: &mut Ui, cols: &[Col<T>], total: usize) {
        let mut remove = None;
        let mut clear = false;
        let mut edit = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(egui::RichText::new(icon::FUNNEL).color(theme::ACCENT));
            ui.label(theme::medium(format!("{} di {}", self.shown, total)).size(12.5));
            if !self.search_all.trim().is_empty() && ui.small_button(format!("\"{}\" {}", self.search_all.trim(), icon::X)).clicked() {
                self.search_all.clear();
            }
            for (n, f) in self.filters.iter().enumerate().filter(|(_, f)| f.is_set()) {
                let title = cols.iter().find(|c| c.id == f.col).map(|c| c.title).unwrap_or("");
                if ui.small_button(format!("{}: {} {}", title, f.summary(), icon::X)).on_hover_text("Togli questa condizione").clicked() {
                    remove = Some(n);
                }
            }
            if ui.link("Modifica").clicked() {
                edit = true;
            }
            if ui.link("Azzera").clicked() {
                clear = true;
            }
        });
        if let Some(n) = remove {
            self.filters.remove(n);
        }
        if clear {
            self.filters.clear();
            self.search_all.clear();
        }
        if edit {
            self.filters_open = true;
        }
    }

    /// Indici delle righe in ordine (gruppo, poi ordinamento scelto), filtri applicati.
    fn sorted<T>(&self, cols: &[Col<T>], rows: &[T]) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..rows.len()).filter(|i| !self.filtering() || self.passes(cols, &rows[*i])).collect();
        let sort = self.prefs.sort.as_ref().and_then(|(id, desc)| cols.iter().find(|c| c.id == id).map(|c| (c, *desc)));
        let group = self.prefs.group.as_ref().and_then(|id| cols.iter().find(|c| c.id == id));
        if sort.is_none() && group.is_none() {
            return idx;
        }
        let keys: Vec<(Option<Val>, Option<Val>)> = rows.iter().map(|r| (group.map(|g| (g.value)(r)), sort.map(|(c, _)| (c.value)(r)))).collect();
        idx.sort_by(|a, b| {
            let (ga, sa) = &keys[*a];
            let (gb, sb) = &keys[*b];
            let g = match (ga, gb) {
                (Some(x), Some(y)) => x.compare(y),
                _ => Ordering::Equal,
            };
            g.then_with(|| match (sa, sb, sort) {
                (Some(x), Some(y), Some((_, desc))) => {
                    let o = x.compare(y);
                    if desc { o.reverse() } else { o }
                }
                _ => Ordering::Equal,
            })
        });
        idx
    }

    fn lines<T>(&self, cols: &[Col<T>], rows: &[T], order: &[usize], visible: &[usize]) -> Vec<Line> {
        let Some(g) = self.prefs.group.as_ref().and_then(|id| cols.iter().find(|c| c.id == id)) else {
            return order.iter().map(|i| Line::Item(*i)).collect();
        };
        let mut out = Vec::new();
        let mut i = 0;
        while i < order.len() {
            let label = (g.value)(&rows[order[i]]).text();
            let mut j = i;
            while j < order.len() && (g.value)(&rows[order[j]]).text() == label {
                j += 1;
            }
            let sums = visible
                .iter()
                .map(|c| cols[*c].total.then(|| order[i..j].iter().filter_map(|r| (cols[*c].value)(&rows[*r]).number()).sum()))
                .collect();
            let label = if label.trim().is_empty() { "(vuoto)".to_string() } else { label };
            let collapsed = self.collapsed.contains(&label);
            out.push(Line::Group { label, count: j - i, sums });
            if !collapsed {
                out.extend(order[i..j].iter().map(|r| Line::Item(*r)));
            }
            i = j;
        }
        out
    }

    /// Pulsanti Colonne, Raggruppa ed Excel (da mettere nella barra dell'elenco).
    pub fn toolbar<T>(&mut self, ui: &mut Ui, store: &Store, msgs: &mut Msgs, cols: &[Col<T>], rows: &[T], title: &str) {
        self.load(store);
        let mut changed = false;
        let active = self.filters.iter().filter(|f| f.is_set()).count() + (!self.search_all.trim().is_empty()) as usize;
        let label = if active > 0 { format!("Filtri ({})", active) } else { "Filtri".to_string() };
        if theme::button(ui, icon::FUNNEL, &label).on_hover_text("Filtra per qualsiasi dato: testo, numeri, importi e periodi").clicked() {
            self.filters_open = !self.filters_open;
            if self.filters_open && self.filters.is_empty() {
                let col = cols.iter().find(|c| Self::kind_of(*c, rows) == Kind::Date).or(cols.first()).map(|c| c.id.to_string()).unwrap_or_default();
                self.filters.push(Filter { col, ..Default::default() });
            }
        }
        if self.filters_open {
            self.filter_window(ui.ctx(), cols, rows, title);
        }
        egui::menu::menu_custom_button(ui, theme::button_widget(icon::COLUMNS, "Colonne"), |ui| {
            ui.set_min_width(260.0);
            ui.label(theme::muted("Mostra, nascondi e sposta le colonne").size(12.0));
            let order = self.ordered(cols);
            let mut moved: Option<(usize, i32)> = None;
            for (pos, i) in order.iter().enumerate() {
                let c = &cols[*i];
                ui.horizontal(|ui| {
                    let mut v = self.visible(c);
                    if ui.checkbox(&mut v, c.title).changed() {
                        self.set_visible(c, v);
                        changed = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(pos + 1 < order.len(), egui::Button::new(icon::CARET_DOWN).small()).on_hover_text("Sposta a destra").clicked() {
                            moved = Some((pos, 1));
                        }
                        if ui.add_enabled(pos > 0, egui::Button::new(icon::CARET_UP).small()).on_hover_text("Sposta a sinistra").clicked() {
                            moved = Some((pos, -1));
                        }
                    });
                });
            }
            if let Some((pos, d)) = moved {
                let mut ids: Vec<String> = order.iter().map(|i| cols[*i].id.to_string()).collect();
                let to = (pos as i32 + d) as usize;
                ids.swap(pos, to);
                self.prefs.order = ids;
                changed = true;
            }
            ui.separator();
            if ui.button(format!("{}  Ripristina l'elenco predefinito", icon::ARROW_COUNTER_CLOCKWISE)).clicked() {
                self.prefs = Prefs::default();
                self.collapsed.clear();
                changed = true;
                ui.close_menu();
            }
        });
        let groupable: Vec<&Col<T>> = cols.iter().filter(|c| c.group).collect();
        if !groupable.is_empty() {
            let current = self.prefs.group.as_ref().and_then(|id| cols.iter().find(|c| c.id == id)).map(|c| c.title).unwrap_or("nessuno");
            egui::ComboBox::from_id_salt(("group", self.name)).selected_text(format!("{} Raggruppa: {}", icon::TREE_STRUCTURE, current)).width(210.0).show_ui(ui, |ui| {
                if ui.selectable_label(self.prefs.group.is_none(), "nessuno").clicked() {
                    self.prefs.group = None;
                    changed = true;
                }
                for c in &groupable {
                    if ui.selectable_label(self.prefs.group.as_deref() == Some(c.id), c.title).clicked() {
                        self.prefs.group = Some(c.id.into());
                        self.collapsed.clear();
                        changed = true;
                    }
                }
            });
        }
        if theme::button(ui, icon::MICROSOFT_EXCEL_LOGO, "Excel").on_hover_text("Esporta in Excel l'elenco così come lo vedi (colonne, ordine e filtro)").clicked() {
            let file = format!("{}.xlsx", title.replace(['/', '\\', ':'], "-"));
            if let Some(p) = rfd::FileDialog::new().add_filter("Excel", &["xlsx"]).set_file_name(&file).save_file() {
                let vis: Vec<usize> = self.ordered(cols).into_iter().filter(|i| self.visible(&cols[*i])).collect();
                let order = self.sorted(cols, rows);
                let mut xcols: Vec<XColumn> = Vec::new();
                if let Some(g) = self.prefs.group.as_ref().and_then(|id| cols.iter().find(|c| c.id == id)) {
                    if !vis.iter().any(|i| cols[*i].id == g.id) {
                        xcols.push(XColumn { title: g.title.into(), width: (g.width / 7.0) as f64, total: false });
                    }
                }
                let group_extra = !xcols.is_empty();
                xcols.extend(vis.iter().map(|i| XColumn { title: cols[*i].title.into(), width: (cols[*i].width / 7.0).max(8.0) as f64, total: cols[*i].total }));
                let data: Vec<Vec<XValue>> = order
                    .iter()
                    .map(|r| {
                        let mut v = Vec::new();
                        if group_extra {
                            let g = self.prefs.group.as_ref().and_then(|id| cols.iter().find(|c| c.id == id)).unwrap();
                            v.push((g.value)(&rows[*r]).excel());
                        }
                        v.extend(vis.iter().map(|i| (cols[*i].value)(&rows[*r]).excel()));
                        v
                    })
                    .collect();
                match openfatt_interop::table::export_table(&p, title, title, &xcols, &data) {
                    Ok(_) => msgs.ok(format!("{} righe esportate in {}", data.len(), p.display())),
                    Err(e) => msgs.err(e),
                }
            }
        }
        if changed {
            self.save(store);
        }
    }

    /// L'elenco. `key` identifica la riga (per la selezione); `height` è lo spazio disponibile.
    #[allow(clippy::too_many_arguments)]
    pub fn show<T>(&mut self, ui: &mut Ui, store: &Store, cols: &[Col<T>], rows: &[T], key: impl Fn(&T) -> i64, height: f32, checkboxes: bool, row_h: f32) -> Events {
        self.load(store);
        let mut ev = Events::default();
        let vis: Vec<usize> = self.ordered(cols).into_iter().filter(|i| self.visible(&cols[*i])).collect();
        let ckey = format!("{}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{}", self.version, rows.len(), self.prefs.sort, self.prefs.group, self.collapsed, vis, self.filters, self.search_all);
        let fresh = self.version != 0 && self.cache.as_ref().map(|c| c.key == ckey).unwrap_or(false);
        if !fresh {
            let order = self.sorted(cols, rows);
            let lines = self.lines(cols, rows, &order, &vis);
            let keys: Vec<i64> = order.iter().map(|i| key(&rows[*i])).collect();
            let totals = vis.iter().filter(|i| cols[**i].total).map(|i| (*i, order.iter().filter_map(|r| (cols[*i].value)(&rows[*r]).number()).sum())).collect();
            self.cache = Some(Cached { key: ckey, order, lines, keys, totals });
        }
        let cached = self.cache.take().unwrap();
        let (order, lines, keys_in_order) = (&cached.order, &cached.lines, &cached.keys);
        self.shown = order.len();
        let mut height = height;
        if self.filtering() {
            self.active_filters(ui, cols, rows.len());
            height -= 26.0;
        }
        if !self.selected.is_empty() {
            let present: std::collections::HashSet<i64> = keys_in_order.iter().copied().collect();
            self.selected.retain(|k| present.contains(k));
        }
        let mut sort_click: Option<&'static str> = None;
        let mut toggle_group: Option<String> = None;
        let mut click: Option<(i64, egui::Modifiers)> = None;
        let mut check: Option<(i64, bool)> = None;
        let mut all: Option<bool> = None;
        let totals_h = if cols.iter().any(|c| c.total) { 26.0 } else { 0.0 };
        // il nome del gruppo va nella colonna per cui si raggruppa (se visibile), altrimenti
        // nella prima colonna di testo abbastanza larga
        let label_col = self
            .prefs
            .group
            .as_ref()
            .and_then(|g| vis.iter().position(|i| cols[*i].id == g))
            .or_else(|| vis.iter().position(|i| !cols[*i].total && !cols[*i].right && cols[*i].width >= 150.0))
            .unwrap_or(0);
        // riga selezionata ben visibile: fondo blu, testo bianco
        let saved_selection = ui.visuals().selection;
        ui.visuals_mut().selection.bg_fill = theme::ROW_SELECTED;
        ui.visuals_mut().selection.stroke.color = egui::Color32::WHITE;
        let mut tb = TableBuilder::new(ui)
            .id_salt(("grid", self.name))
            .striped(false)
            .resizable(true)
            .sense(egui::Sense::click())
            .max_scroll_height((height - totals_h).max(80.0))
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        if checkboxes {
            tb = tb.column(Column::exact(34.0));
        }
        for (n, i) in vis.iter().enumerate() {
            let c = &cols[*i];
            let _ = n;
            tb = tb.column(Column::initial(c.width).at_least(40.0).clip(true));
        }
        // colonna vuota finale: le altre restano della loro larghezza
        tb = tb.column(Column::remainder().at_least(0.0));
        tb.header(24.0, |mut h| {
            if checkboxes {
                h.col(|ui| {
                    let mut a = !order.is_empty() && self.selected.len() == order.len();
                    if ui.checkbox(&mut a, "").on_hover_text("Seleziona tutti").changed() {
                        all = Some(a);
                    }
                });
            }
            for i in &vis {
                let c = &cols[*i];
                h.col(|ui| {
                    let arrow = match &self.prefs.sort {
                        Some((id, desc)) if id == c.id => if *desc { format!(" {}", icon::ARROW_DOWN) } else { format!(" {}", icon::ARROW_UP) },
                        _ => String::new(),
                    };
                    let layout = if c.right { egui::Layout::right_to_left(egui::Align::Center) } else { egui::Layout::left_to_right(egui::Align::Center) };
                    ui.with_layout(layout, |ui| {
                        if c.right {
                            ui.add_space(8.0);
                        }
                        if ui.add(egui::Label::new(theme::medium(format!("{}{}", c.title, arrow)).size(12.5).color(theme::INK_SOFT)).sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Ordina").clicked() {
                            sort_click = Some(c.id);
                        }
                    });
                });
            }
            h.col(|_| {});
        })
        .body(|body| {
            body.rows(row_h, lines.len(), |mut row| {
                match &lines[row.index()] {
                    Line::Group { label, count, sums } => {
                        let collapsed = self.collapsed.contains(label);
                        if checkboxes {
                            row.col(|_| {});
                        }
                        for (n, _) in vis.iter().enumerate() {
                            row.col(|ui| {
                                if n == label_col {
                                    ui.add(egui::Label::new(theme::bold(format!("{} {}  ({})", if collapsed { icon::CARET_RIGHT } else { icon::CARET_DOWN }, label, count))).truncate());
                                } else if let Some(s) = sums[n] {
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.add_space(8.0);
                                        ui.label(theme::bold(eur(s)));
                                    });
                                }
                            });
                        }
                        row.col(|_| {});
                        if row.response().clicked() {
                            toggle_group = Some(label.clone());
                        }
                    }
                    Line::Item(r) => {
                        let t = &rows[*r];
                        let k = key(t);
                        let sel = self.selected.contains(&k);
                        row.set_selected(sel && !self.keep_selection || self.keep_selection && self.anchor == Some(k));
                        if checkboxes {
                            row.col(|ui| {
                                let mut s = sel;
                                if ui.checkbox(&mut s, "").changed() {
                                    check = Some((k, s));
                                }
                            });
                        }
                        for i in &vis {
                            let c = &cols[*i];
                            row.col(|ui| {
                                if let Some(f) = &c.render {
                                    f(ui, t);
                                } else {
                                    let v = (c.value)(t).text();
                                    if let Some(r) = c.opens.as_ref().and_then(|o| o(t)) {
                                        super::quick::link(ui, v, r);
                                    } else if c.right {
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.add_space(8.0);
                                            ui.label(v);
                                        });
                                    } else {
                                        ui.add(egui::Label::new(v).truncate());
                                    }
                                }
                            });
                        }
                        row.col(|_| {});
                        let resp = row.response();
                        if resp.double_clicked() {
                            ev.opened = Some(k);
                        } else if resp.clicked() {
                            click = Some((k, resp.ctx.input(|i| i.modifiers)));
                        }
                    }
                }
            });
        });
        ui.visuals_mut().selection = saved_selection;
        // totali sotto l'elenco
        let total_cols: Vec<&Col<T>> = vis.iter().map(|i| &cols[*i]).filter(|c| c.total).collect();
        if !total_cols.is_empty() {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                let sel_rows: Vec<&T> = if self.selected.len() > 1 { rows.iter().filter(|r| self.selected.contains(&key(r))).collect() } else { Vec::new() };
                for c in &total_cols {
                    let s: Decimal = cached.totals.iter().find(|(i, _)| cols[*i].id == c.id).map(|(_, v)| *v).unwrap_or_default();
                    ui.label(theme::muted(format!("{}:", c.title)));
                    ui.label(theme::medium(eur(s)));
                    if sel_rows.len() > 1 {
                        let ss: Decimal = sel_rows.iter().filter_map(|r| (c.value)(r).number()).sum();
                        ui.label(theme::muted(format!("(selezionati {})", eur(ss))));
                    }
                    ui.add_space(14.0);
                }
            });
        }
        // azioni
        let mut changed = false;
        if let Some(id) = sort_click {
            self.prefs.sort = match &self.prefs.sort {
                Some((cur, false)) if cur == id => Some((id.into(), true)),
                Some((cur, true)) if cur == id => None,
                _ => Some((id.into(), false)),
            };
            changed = true;
        }
        if let Some(g) = toggle_group {
            if !self.collapsed.remove(&g) {
                self.collapsed.insert(g);
            }
        }
        if let Some(a) = all {
            self.selected = if a { keys_in_order.iter().copied().collect() } else { BTreeSet::new() };
        }
        if let Some((k, s)) = check {
            if s {
                self.selected.insert(k);
            } else {
                self.selected.remove(&k);
            }
            self.anchor = Some(k);
        }
        if let Some((k, m)) = click {
            if self.keep_selection {
                self.anchor = Some(k);
            } else if m.shift {
                if let (Some(a), Some(pk)) = (self.anchor.and_then(|a| keys_in_order.iter().position(|x| *x == a)), keys_in_order.iter().position(|x| *x == k)) {
                    let (lo, hi) = if a <= pk { (a, pk) } else { (pk, a) };
                    if !m.command {
                        self.selected.clear();
                    }
                    self.selected.extend(keys_in_order[lo..=hi].iter().copied());
                }
            } else if m.command {
                if !self.selected.remove(&k) {
                    self.selected.insert(k);
                }
                self.anchor = Some(k);
            } else {
                self.selected.clear();
                self.selected.insert(k);
                self.anchor = Some(k);
            }
        }
        self.cache = Some(cached);
        if changed {
            self.save(store);
        }
        ev
    }

    /// L'unico elemento selezionato, se ce n'è uno solo.
    pub fn single(&self) -> Option<i64> {
        (self.selected.len() == 1).then(|| *self.selected.iter().next().unwrap())
    }

    pub fn select_only(&mut self, k: i64) {
        self.selected.clear();
        self.selected.insert(k);
        self.anchor = Some(k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    struct R {
        name: &'static str,
        city: &'static str,
        amount: Decimal,
        date: NaiveDate,
    }

    fn rows() -> Vec<R> {
        let d = |m, g| NaiveDate::from_ymd_opt(2026, m, g).unwrap();
        vec![
            R { name: "bianchi", city: "Trento", amount: dec!(100), date: d(3, 2) },
            R { name: "Rossi", city: "Roma", amount: dec!(9.5), date: d(1, 15) },
            R { name: "Verdi", city: "Trento", amount: dec!(1000), date: d(2, 1) },
        ]
    }

    fn cols<'a>() -> Vec<Col<'a, R>> {
        vec![
            Col::text("name", "Nome", 100.0, |r: &R| r.name.to_string()),
            Col::text("city", "Comune", 100.0, |r: &R| r.city.to_string()).group(),
            Col::money("amount", "Importo", 80.0, |r: &R| r.amount).total(),
            Col::new("date", "Data", 80.0, |r: &R| Val::Date(Some(r.date))).hidden(),
        ]
    }

    #[test]
    fn ordinamento_numeri_testo_date() {
        let (rows, cols) = (rows(), cols());
        let mut g = Grid::new("prova");
        assert_eq!(g.sorted(&cols, &rows), vec![0, 1, 2]);
        g.prefs.sort = Some(("amount".into(), false));
        assert_eq!(g.sorted(&cols, &rows), vec![1, 0, 2]); // 9,5 < 100 < 1000 (non alfabetico)
        g.prefs.sort = Some(("amount".into(), true));
        assert_eq!(g.sorted(&cols, &rows), vec![2, 0, 1]);
        g.prefs.sort = Some(("name".into(), false));
        assert_eq!(g.sorted(&cols, &rows), vec![0, 1, 2]); // senza distinzione di maiuscole
        g.prefs.sort = Some(("date".into(), false));
        assert_eq!(g.sorted(&cols, &rows), vec![1, 2, 0]);
    }

    #[test]
    fn gruppi_con_subtotali_e_chiusura() {
        let (rows, cols) = (rows(), cols());
        let mut g = Grid::new("prova");
        g.prefs.group = Some("city".into());
        g.prefs.sort = Some(("amount".into(), true));
        let order = g.sorted(&cols, &rows);
        assert_eq!(order, vec![1, 2, 0]); // Roma, poi Trento ordinato per importo decrescente
        let vis = vec![0, 1, 2];
        let lines = g.lines(&cols, &rows, &order, &vis);
        assert_eq!(lines.len(), 5);
        match &lines[2] {
            Line::Group { label, count, sums } => {
                assert_eq!((label.as_str(), *count), ("Trento", 2));
                assert_eq!(sums[2], Some(dec!(1100)));
                assert_eq!(sums[0], None);
            }
            _ => panic!("atteso un gruppo"),
        }
        g.collapsed.insert("Trento".into());
        assert_eq!(g.lines(&cols, &rows, &order, &vis).len(), 3);
    }

    #[test]
    fn filtri_per_testo_importo_e_periodo() {
        let (rows, cols) = (rows(), cols());
        let d = |m, g| NaiveDate::from_ymd_opt(2026, m, g).unwrap();
        let mut g = Grid::new("prova");
        assert_eq!(g.sorted(&cols, &rows).len(), 3);
        g.filters.push(Filter { col: "city".into(), text: "tren".into(), ..Default::default() });
        assert_eq!(g.sorted(&cols, &rows), vec![0, 2]);
        g.filters.push(Filter { col: "amount".into(), min: Some(dec!(500)), ..Default::default() });
        assert_eq!(g.sorted(&cols, &rows), vec![2]);
        g.filters.clear();
        // anche su una colonna nascosta
        g.filters.push(Filter { col: "date".into(), from: Some(d(1, 20)), to: Some(d(2, 28)), ..Default::default() });
        assert_eq!(g.sorted(&cols, &rows), vec![2]);
        g.filters.clear();
        g.filters.push(Filter { col: "city".into(), op: TextOp::Equals, text: "ROMA".into(), ..Default::default() });
        assert_eq!(g.sorted(&cols, &rows), vec![1]);
        g.filters[0].op = TextOp::NotContains;
        assert_eq!(g.sorted(&cols, &rows), vec![0, 2]);
        g.filters.clear();
        g.search_all = "verdi trento".into();
        assert_eq!(g.sorted(&cols, &rows), vec![2]);
        assert!(g.filtering() && !g.has_filter("city"));
    }

    #[test]
    fn i_periodi_sono_corretti() {
        let today = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let p: BTreeMap<&str, (NaiveDate, NaiveDate)> = periods(today).into_iter().map(|(n, a, b)| (n, (a, b))).collect();
        let d = |y, m, g| NaiveDate::from_ymd_opt(y, m, g).unwrap();
        assert_eq!(p["Mese scorso"], (d(2025, 12, 1), d(2025, 12, 31)));
        assert_eq!(p["Questo mese"], (d(2026, 1, 1), d(2026, 1, 31)));
        assert_eq!(p["Questo trimestre"], (d(2026, 1, 1), d(2026, 3, 31)));
        assert_eq!(p["Anno scorso"], (d(2025, 1, 1), d(2025, 12, 31)));
        assert_eq!(p["Questa settimana"], (d(2026, 1, 12), d(2026, 1, 18)));
        assert_eq!(p["Ultimi 7 giorni"], (d(2026, 1, 9), today));
    }

    #[test]
    fn colonne_visibili_e_ordine() {
        let cols = cols();
        let mut g = Grid::new("prova");
        assert!(!g.visible(&cols[3]));
        g.set_visible(&cols[3], true);
        g.set_visible(&cols[0], false);
        assert!(g.visible(&cols[3]) && !g.visible(&cols[0]));
        g.prefs.order = vec!["amount".into(), "name".into()];
        assert_eq!(g.ordered(&cols), vec![2, 0, 1, 3]);
        assert_eq!(Val::Money(dec!(1234.5)).text(), "1.234,50");
        assert_eq!(Val::Num(dec!(2.500), 3).text(), "2,5");
    }
}
