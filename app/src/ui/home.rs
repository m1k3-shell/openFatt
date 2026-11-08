//! Scrivania: cosa c'è da fare oggi, incassi in arrivo, fatturato e ultimi documenti.

use chrono::{Datelike, NaiveDate};
use eframe::egui::{self, RichText, Ui, Vec2};
use openfatt_core::model::{DocKind, DocStatus};
use openfatt_store::{Direction, EInvoiceStatus};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use super::theme::{self, icon, Tone, MUTED, NEGATIVE};
use super::widgets::eur;
use super::{Ctx, DocGroup, Nav, Screen};

#[derive(Default)]
pub struct State {
    issues: Option<(String, Vec<openfatt_fatturapa::Issue>)>,
    data: super::Cache<NaiveDate, HomeData>,
}

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

    // ---------------- dati ----------------
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
    let incoming = &hd.incoming;
    let without_fe = &hd.without_fe;
    let dues = &hd.dues;
    let receivables: Vec<_> = dues.iter().filter(|d| d.receivable && d.residual() > Decimal::ZERO).collect();
    let overdue: Vec<_> = receivables.iter().filter(|d| d.date < today).collect();
    let overdue_sum: Decimal = overdue.iter().map(|d| d.residual()).sum();
    let open_sum: Decimal = receivables.iter().map(|d| d.residual()).sum();
    let in30: Decimal = receivables.iter().filter(|d| d.date >= today && d.date <= today + chrono::Duration::days(30)).map(|d| d.residual()).sum();
    let payables_overdue: Decimal = dues.iter().filter(|d| !d.receivable && d.date < today).map(|d| d.residual()).sum();
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
    // solleciti di pagamento pronti da inviare
    let tpl = crate::mailer::templates(s);
    let to_remind: Vec<_> = s.reminder_candidates(today, tpl.grace_days, tpl.min_days_between).unwrap_or_default().into_iter().filter(|c| !c.recently_reminded).collect();
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
    if let Ok(vs) = s.vat_settings() {
        let cur = openfatt_core::vat::Period::containing(today, vs.periodicity);
        let last = if cur.index > 1 { openfatt_core::vat::Period { index: cur.index - 1, ..cur } } else { openfatt_core::vat::Period::new(cur.year - 1, cur.count(), vs.periodicity) };
        let due = last.payment_due();
        if due >= today - chrono::Duration::days(60) && due <= today + chrono::Duration::days(12) {
            if let Some((_, l)) = s.vat_liquidations(last.year, last.index).ok().and_then(|y| y.into_iter().last()) {
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
        let list: Vec<_> = s.bank_pending(kind).unwrap_or_default().into_iter().filter(|d| d.date <= today + chrono::Duration::days(days)).collect();
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
            go: Go::Nav(Nav::Go(Screen::Dues)),
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
    for id in without_fe.iter().take(4) {
        if let Ok(d) = s.document(*id) {
            todo.push(Todo {
                icon: icon::PAPER_PLANE_TILT,
                bad: false,
                title: format!("{} {} da trasmettere allo SdI", d.kind.label(), d.number_label()),
                desc: format!("{} · emessa il {}", d.party.display_name(), d.date.format("%d/%m/%Y")),
                amount: openfatt_core::calc::compute(&d).ok().map(|t| t.total),
                action_icon: icon::FILE_ARROW_UP,
                action: "Genera XML",
                go: Go::Generate(*id),
            });
        }
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
        let t = s.inventory_totals(inv.id).unwrap_or_default();
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

    // documenti ricorrenti, agenda
    let rec = s.due_recurrences(today).unwrap_or_default();
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
    let agenda: Vec<_> = s.agenda(today, today).unwrap_or_default().into_iter().filter(|a| !a.done).collect();
    let late = s.agenda_overdue(today).unwrap_or_default();
    if !agenda.is_empty() || !late.is_empty() {
        let first = agenda.first().map(|a| format!("{}{}", if a.time.is_empty() { String::new() } else { format!("{} ", a.time) }, a.title)).unwrap_or_default();
        todo.push(Todo {
            icon: icon::CALENDAR_BLANK,
            bad: !late.is_empty(),
            title: if agenda.is_empty() { format!("{} attività arretrate in agenda", late.len()) } else { format!("Oggi in agenda: {}", first) },
            desc: match (agenda.len(), late.len()) {
                (0, _) => format!("la più vecchia del {}", late.first().map(|a| a.date.format("%d/%m/%Y").to_string()).unwrap_or_default()),
                (n, 0) => if n > 1 { format!("e altri {} impegni", n - 1) } else { "nessun altro impegno".into() },
                (n, l) => format!("{} impegni oggi, {} attività arretrate", n, l),
            },
            amount: None,
            action_icon: icon::CALENDAR_BLANK,
            action: "Agenda",
            go: Go::Nav(Nav::Go(Screen::Extra(super::business::Extra::Agenda))),
        });
    }

    // ---------------- titolo ----------------
    let mut parts: Vec<(String, bool)> = vec![(long_date(today), false)];
    if !rejected.is_empty() {
        parts.push((" · ".into(), false));
        parts.push((if rejected.len() == 1 { "una fattura scartata dallo SdI".into() } else { format!("{} fatture scartate dallo SdI", rejected.len()) }, true));
    }
    if overdue_sum > Decimal::ZERO {
        parts.push((" · ".into(), false));
        parts.push((format!("€ {} di crediti scaduti", eur(overdue_sum)), true));
    }
    if rejected.is_empty() && overdue_sum.is_zero() {
        parts.push((" · ".into(), false));
        parts.push((if todo.is_empty() { "nessuna attività in sospeso".into() } else { format!("{} attività da completare", todo.len()) }, false));
    }
    let sub = theme::subline(&parts.iter().map(|(t, b)| (t.as_str(), *b)).collect::<Vec<_>>());
    theme::title_row(ui, &[], "Start", Some(sub), |ui| {
        if theme::primary(ui, icon::PLUS, "Nuova fattura").clicked() {
            cx.nav.push(Nav::NewDocument(DocKind::Invoice));
        }
        ui.scope(|ui| {
            let b = theme::button_widget(icon::CARET_DOWN, "Altri documenti");
            egui::menu::menu_custom_button(ui, b, |ui| {
                for k in [DocKind::Quote, DocKind::CustomerOrder, DocKind::DeliveryNote, DocKind::AccompanyingInvoice, DocKind::CreditNote, DocKind::Proforma, DocKind::SupplierOrder, DocKind::GoodsReceipt, DocKind::PurchaseInvoice] {
                    if ui.button(format!("{}  {}", icon::PLUS, k.label())).clicked() {
                        cx.nav.push(Nav::NewDocument(k));
                        ui.close_menu();
                    }
                }
            });
        });
        if theme::button(ui, icon::CASH_REGISTER, "Vendita al banco").on_hover_text("F9").clicked() {
            cx.nav.push(Nav::Go(Screen::Counter));
        }
    });

    // ---------------- colonne ----------------
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let gap = 12.0;
        let total_w = ui.available_width();
        let left_w = ((total_w - gap) * 2.0 / 3.0).floor();
        let right_w = total_w - gap - left_w;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(gap, gap);
            ui.allocate_ui_with_layout(Vec2::new(left_w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(left_w);
                // Da fare
                let n = todo.len();
                theme::card(ui, "Da fare", |ui| {
                    ui.label(RichText::new(if n == 1 { "1 attività".to_string() } else { format!("{} attività", n) }).size(12.0).color(MUTED));
                }, |ui| {
                    if todo.is_empty() {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon::CHECK_CIRCLE).size(20.0).color(theme::POSITIVE));
                            ui.label(theme::muted("Tutto in ordine: nessuna fattura da inviare, nessuno scarto, nessuna scadenza passata."));
                        });
                    }
                    for (i, t) in todo.iter().enumerate() {
                        if i > 0 {
                            theme::rule(ui);
                        }
                        if todo_row(ui, t) {
                            match &t.go {
                                Go::Nav(n) => cx.nav.push(clone_nav(n)),
                                Go::Generate(id) => {
                                    super::documents::generate_einvoices(cx, &[*id], &mut st.issues);
                                    cx.nav.push(Nav::Go(Screen::EInvoicesOut));
                                }
                            }
                        }
                    }
                });
                // Ultimi documenti
                let mut go_all = false;
                theme::card(ui, "Ultimi documenti", |ui| {
                    if ui.link(RichText::new(format!("Tutte le fatture  {}", icon::ARROW_RIGHT)).color(theme::INK)).clicked() {
                        go_all = true;
                    }
                }, |ui| {
                    if recent_docs.is_empty() {
                        ui.label(theme::muted("Nessun documento negli ultimi sei mesi."));
                    }
                    egui::Grid::new("recent").num_columns(5).spacing([16.0, 0.0]).min_row_height(36.0).show(ui, |ui| {
                        for h in ["Documento", "Data", "Cliente / fornitore", "Totale", "Stato"] {
                            ui.label(RichText::new(h).size(12.0).color(MUTED));
                        }
                        ui.end_row();
                        for d in recent_docs.iter().take(6) {
                            if ui.add(egui::Label::new(theme::medium(format!("{} {}", d.kind.label(), d.number_label))).sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                cx.nav.push(Nav::OpenDocument(d.id));
                            }
                            ui.label(d.date.format("%d/%m/%Y").to_string());
                            ui.label(if d.party_name.is_empty() { "-".to_string() } else { truncate(&d.party_name, 34) });
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
                });
                if go_all {
                    cx.nav.push(Nav::Go(Screen::Documents(DocGroup::Invoices)));
                }
            });
            ui.allocate_ui_with_layout(Vec2::new(right_w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(right_w);
                theme::card(ui, "Incassi", |ui| {
                    ui.label(RichText::new(format!("al {} {}", today.day(), MONTHS[today.month0() as usize])).size(12.0).color(MUTED));
                }, |ui| {
                    ui.columns(2, |c| {
                        c[0].label(theme::muted("Da incassare").size(13.0));
                        c[0].label(theme::bold(eur(open_sum)).size(28.0));
                        c[1].label(theme::muted("di cui scaduti").size(13.0));
                        let col = if overdue_sum.is_zero() { MUTED } else { NEGATIVE };
                        c[1].label(RichText::new(eur(overdue_sum)).size(22.0).family(theme::medium_family()).color(col));
                    });
                    ui.add_space(6.0);
                    let mut list: Vec<_> = receivables.clone();
                    list.sort_by_key(|d| d.date);
                    if list.is_empty() {
                        ui.label(theme::muted("Nessuna scadenza aperta."));
                    }
                    for d in list.iter().take(5) {
                        theme::rule(ui);
                        let late = d.date < today;
                        ui.horizontal(|ui| {
                            ui.set_min_height(44.0);
                            ui.vertical(|ui| {
                                ui.set_width(40.0);
                                ui.spacing_mut().item_spacing.y = 0.0;
                                let c = if late { NEGATIVE } else { theme::INK };
                                ui.label(RichText::new(format!("{:02}", d.date.day())).family(theme::medium_family()).color(c));
                                ui.label(RichText::new(MONTHS_SHORT[d.date.month0() as usize]).size(12.0).color(if late { NEGATIVE } else { MUTED }));
                            });
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.set_width((ui.available_width() - 90.0).max(60.0));
                                ui.label(truncate(&d.party_name, 28));
                                ui.label(RichText::new(format!("{}{}", d.doc_label, if late { " · scaduta" } else { "" })).size(12.0).color(MUTED));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(eur(d.residual()));
                            });
                        });
                    }
                    theme::rule(ui);
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(theme::muted(format!("Entro 30 giorni: € {}", eur(in30))));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.link(RichText::new(format!("Scadenzario  {}", icon::ARROW_RIGHT)).color(theme::INK)).clicked() {
                                cx.nav.push(Nav::Go(Screen::Dues));
                            }
                        });
                    });
                    if payables_overdue > Decimal::ZERO {
                        ui.label(RichText::new(format!("Pagamenti a fornitori scaduti: € {}", eur(payables_overdue))).color(NEGATIVE));
                    }
                });
                // fatturato degli ultimi sei mesi
                let mut labels = Vec::new();
                let mut values = Vec::new();
                let mut year_total = Decimal::ZERO;
                for i in 0..6 {
                    let m0 = first_month.month0() as i32 + i;
                    let (y, m) = (first_month.year() + m0 / 12, (m0 % 12) as u32);
                    labels.push(MONTHS_SHORT[m as usize].to_string());
                    let v: Decimal = recent_docs
                        .iter()
                        .filter(|d| d.status == DocStatus::Issued && d.date.year() == y && d.date.month0() == m)
                        .filter_map(|d| sales_sign(d.kind).map(|k| k * d.total))
                        .sum();
                    values.push(v.to_f64().unwrap_or(0.0));
                }
                let year_docs = &hd.year_docs;
                for d in year_docs.iter() {
                    if let Some(k) = sales_sign(d.kind) {
                        year_total += k * d.total;
                    }
                }
                theme::card(ui, "Fatturato", |ui| {
                    ui.label(RichText::new("fatture e banco, IVA inclusa").size(12.0).color(MUTED));
                }, |ui| {
                    theme::bar_chart(ui, &labels, &values, 170.0, |v| {
                        if v >= 1000.0 {
                            format!("{}", eur(Decimal::from_f64_retain(v).unwrap_or_default().round()).trim_end_matches(",00"))
                        } else {
                            format!("{}", v.round())
                        }
                    });
                    ui.add_space(8.0);
                    ui.label(theme::muted(format!("Totale {}: € {} · {} in corso", today.year(), eur(year_total), MONTHS[today.month0() as usize])).size(13.0));
                });
            });
        });
    });
}

fn todo_row(ui: &mut Ui, t: &Todo) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.set_min_height(56.0);
        ui.add_sized([28.0, 28.0], egui::Label::new(RichText::new(t.icon).size(20.0).color(if t.bad { NEGATIVE } else { theme::INK_SOFT })));
        let w = ui.available_width();
        ui.vertical(|ui| {
            ui.set_width((w - 320.0).max(160.0));
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(egui::Label::new(theme::medium(&t.title)).truncate());
            ui.add(egui::Label::new(RichText::new(&t.desc).size(13.0).color(MUTED)).truncate());
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(theme::button_widget(t.action_icon, t.action).min_size(Vec2::new(176.0, 36.0))).clicked() {
                clicked = true;
            }
            ui.add_space(8.0);
            match t.amount {
                Some(a) => ui.label(eur(a)),
                None => ui.label(RichText::new("-").color(MUTED)),
            };
        });
    });
    clicked
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
        _ => Nav::Go(Screen::Home),
    }
}
