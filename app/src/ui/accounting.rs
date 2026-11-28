//! Prima nota avanzata: piano dei conti con transcodifica per lo studio, saldi iniziali,
//! giroconti, riconciliazione dell'estratto conto (camt.053 o CSV), versamenti F24 ed
//! esportazione per il commercialista.

use chrono::{Datelike, NaiveDate};
use eframe::egui::{self, RichText};
use openfatt_store::{AccountKind, ChartOfAccounts, F24Item, LedgerAccount, ReconLine, ReconMatch};
use rust_decimal::Decimal;

use super::theme::{self, icon};
use super::widgets::{self, eur, GRAY, GREEN, ORANGE, RED};
use super::Ctx;

const MONTHS: [&str; 12] = ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"];

fn month_name(m: u32) -> &'static str {
    MONTHS.get((m.max(1) - 1) as usize).copied().unwrap_or("")
}

#[derive(Default)]
pub struct State {
    chart: Option<ChartOfAccounts>,
    transfer: Option<(String, String, NaiveDate, Decimal, String)>,
    opening: Option<(String, NaiveDate, Decimal)>,
    recon: Option<(String, Vec<(bool, ReconLine)>, String)>,
    f24: Option<(i32, u32, Vec<F24Item>, NaiveDate, String)>,
    export: Option<(NaiveDate, NaiveDate)>,
}

fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

impl State {
    /// Riconciliazione già aperta su un estratto conto (collaudi e catture dello schermo).
    pub fn with_statement(store: &openfatt_store::Store, file: &std::path::Path, account: &str) -> Self {
        let mut st = State::default();
        if let Ok(s) = std::fs::read(file).map_err(|e| e.to_string()).and_then(|b| openfatt_bank::camt::parse_statement(&String::from_utf8_lossy(&b))) {
            if let Ok(r) = store.reconcile(account, &s.lines) {
                st.recon = Some((account.to_string(), r.into_iter().map(|l| (!matches!(l.matched, ReconMatch::Recorded(_)), l)).collect(), format!("{} movimenti · IBAN {}", s.lines.len(), s.iban)));
            }
        }
        st
    }

    pub fn with_chart(store: &openfatt_store::Store) -> Self {
        State { chart: Some(store.chart_of_accounts().unwrap_or_default()), ..Default::default() }
    }

    pub fn with_f24(store: &openfatt_store::Store, cx_year: i32, month: u32) -> Self {
        let mut st = State { f24: Some((cx_year, month, Vec::new(), chrono::Local::now().date_naive(), "Banca".into())), ..Default::default() };
        if let Ok(p) = store.f24_proposals(cx_year, month) {
            if let Some((_, _, items, date, _)) = st.f24.as_mut() {
                *items = p.iter().flat_map(|x| x.items.clone()).collect();
                if let Some(f) = p.first() {
                    *date = f.due;
                }
            }
        }
        st
    }
}

/// Pulsanti della barra della prima nota.
pub fn toolbar(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, accounts: &[String]) {
    let first = accounts.first().cloned().unwrap_or_else(|| "Cassa".into());
    let bank = accounts.iter().find(|a| a.to_lowercase().contains("banc")).cloned().unwrap_or_else(|| "Banca".into());
    if ui.button(format!("{}  Giroconto", icon::ARROWS_LEFT_RIGHT)).on_hover_text("Prelievo, versamento, ricarica: da un conto all'altro").clicked() {
        st.transfer = Some((first.clone(), bank.clone(), today(), Decimal::ZERO, String::new()));
    }
    if ui.button("Saldo iniziale").on_hover_text("Saldo di un conto alla data di inizio").clicked() {
        st.opening = Some((first.clone(), NaiveDate::from_ymd_opt(today().year(), 1, 1).unwrap(), Decimal::ZERO));
    }
    if ui.button(format!("{}  Estratto conto...", icon::BANK)).on_hover_text("Riconcilia i movimenti della banca (file camt.053 XML o CSV dell'home banking)").clicked() {
        if let Some(p) = rfd::FileDialog::new().add_filter("Estratto conto", &["xml", "csv", "txt"]).pick_file() {
            match std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| openfatt_bank::camt::parse_statement(&String::from_utf8_lossy(&b))) {
                Ok(s) => {
                    let chart = cx.store.chart_of_accounts().unwrap_or_default();
                    let account = chart.accounts.iter().find(|a| !a.iban.trim().is_empty() && a.iban.replace(' ', "").eq_ignore_ascii_case(&s.iban)).map(|a| a.name.clone()).unwrap_or(bank.clone());
                    let info = format!(
                        "{} movimenti{}{}",
                        s.lines.len(),
                        if s.iban.is_empty() { String::new() } else { format!(" · IBAN {}", s.iban) },
                        match (s.opening, s.closing) {
                            (Some(o), Some(c)) => format!(" · saldo iniziale € {} finale € {}", eur(o), eur(c)),
                            _ => String::new(),
                        }
                    );
                    match cx.store.reconcile(&account, &s.lines) {
                        Ok(r) => st.recon = Some((account, r.into_iter().map(|l| (!matches!(l.matched, ReconMatch::Recorded(_)), l)).collect(), info)),
                        Err(e) => cx.msgs.err(e.to_string()),
                    }
                }
                Err(e) => cx.msgs.err(e),
            }
        }
    }
    if ui.button("F24").on_hover_text("Ritenute e IVA da versare, registro dei versamenti").clicked() {
        let t = today();
        let (y, m) = if t.month() == 1 { (t.year() - 1, 12) } else { (t.year(), t.month() - 1) };
        st.f24 = Some((y, m, Vec::new(), t, bank.clone()));
        load_f24(cx, st);
    }
    if ui.button("Piano dei conti").clicked() {
        st.chart = Some(cx.store.chart_of_accounts().unwrap_or_default());
    }
    if ui.button(format!("{}  Per lo studio...", icon::EXPORT)).on_hover_text("Prima nota e registri IVA in CSV con i codici del commercialista").clicked() {
        let t = today();
        st.export = Some((NaiveDate::from_ymd_opt(t.year(), 1, 1).unwrap(), t));
    }
}

fn load_f24(cx: &mut Ctx, st: &mut State) {
    if let Some((y, m, items, date, _)) = st.f24.as_mut() {
        match cx.store.f24_proposals(*y, *m) {
            Ok(p) => {
                *items = p.iter().flat_map(|x| x.items.clone()).collect();
                if let Some(first) = p.first() {
                    *date = first.due;
                }
            }
            Err(e) => cx.msgs.err(e.to_string()),
        }
    }
}

/// Categorie proposte per i movimenti (voci del piano dei conti).
pub fn category_menu(ui: &mut egui::Ui, store: &openfatt_store::Store, value: &mut String) {
    let chart = store.chart_of_accounts().unwrap_or_default();
    ui.menu_button(icon::CARET_DOWN, |ui| {
        for kind in [AccountKind::Income, AccountKind::Expense] {
            ui.label(RichText::new(kind.label()).small().color(GRAY));
            for a in chart.accounts.iter().filter(|a| a.kind == kind) {
                if ui.button(&a.name).clicked() {
                    *value = a.name.clone();
                    ui.close_menu();
                }
            }
        }
    });
}

pub fn windows(ctx: &egui::Context, cx: &mut Ctx, st: &mut State, accounts: &[String]) {
    transfer_window(ctx, cx, st, accounts);
    opening_window(ctx, cx, st, accounts);
    recon_window(ctx, cx, st);
    f24_window(ctx, cx, st, accounts);
    chart_window(ctx, cx, st);
    export_window(ctx, cx, st);
}

fn account_combo(ui: &mut egui::Ui, id: &str, value: &mut String, accounts: &[String]) {
    egui::ComboBox::from_id_salt(id).width(160.0).selected_text(value.clone()).show_ui(ui, |ui| {
        for a in accounts {
            ui.selectable_value(value, a.clone(), a);
        }
    });
}

fn transfer_window(ctx: &egui::Context, cx: &mut Ctx, st: &mut State, accounts: &[String]) {
    let Some((from, to, date, amount, text)) = st.transfer.as_mut() else { return };
    let mut close = false;
    egui::Window::new("Giroconto").collapsible(false).resizable(false).show(ctx, |ui| {
        egui::Grid::new("giro").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Data");
            widgets::date_field(ui, "giro_date", date);
            ui.end_row();
            ui.label("Dal conto");
            account_combo(ui, "giro_from", from, accounts);
            ui.end_row();
            ui.label("Al conto");
            ui.horizontal(|ui| {
                account_combo(ui, "giro_to", to, accounts);
                ui.add(egui::TextEdit::singleline(to).desired_width(110.0).hint_text("o nuovo conto"));
            });
            ui.end_row();
            ui.label("Importo");
            widgets::dec_field(ui, "giro_amount", amount, 100.0, 2);
            ui.end_row();
            ui.label("Descrizione");
            ui.add(egui::TextEdit::singleline(text).desired_width(260.0).hint_text("es. versamento contanti"));
            ui.end_row();
        });
        ui.label(RichText::new("Due movimenti che si annullano: non sono né entrate né uscite.").small().color(GRAY));
        ui.horizontal(|ui| {
            if theme::primary(ui, icon::CHECK, "Registra").clicked() && cx.msgs.check(cx.store.transfer(from, to, *date, *amount, text), "Giroconto registrato").is_some() {
                close = true;
            }
            if ui.button("Annulla").clicked() {
                close = true;
            }
        });
    });
    if close {
        st.transfer = None;
    }
}

fn opening_window(ctx: &egui::Context, cx: &mut Ctx, st: &mut State, accounts: &[String]) {
    let Some((account, date, amount)) = st.opening.as_mut() else { return };
    let mut close = false;
    egui::Window::new("Saldo iniziale").collapsible(false).resizable(false).show(ctx, |ui| {
        egui::Grid::new("opening").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Conto");
            ui.horizontal(|ui| {
                account_combo(ui, "op_acc", account, accounts);
                ui.add(egui::TextEdit::singleline(account).desired_width(110.0));
            });
            ui.end_row();
            ui.label("Alla data");
            widgets::date_field(ui, "op_date", date);
            ui.end_row();
            ui.label("Saldo €");
            widgets::dec_field(ui, "op_amount", amount, 110.0, 2);
            ui.end_row();
        });
        ui.label(RichText::new("Negativo se il conto è in rosso. Sostituisce il saldo iniziale già inserito per il conto.").small().color(GRAY));
        ui.horizontal(|ui| {
            if theme::primary(ui, icon::CHECK, "Salva").clicked() && cx.msgs.check(cx.store.set_opening_balance(account, *date, *amount), "Saldo iniziale salvato").is_some() {
                close = true;
            }
            if ui.button("Annulla").clicked() {
                close = true;
            }
        });
    });
    if close {
        st.opening = None;
    }
}

fn recon_window(ctx: &egui::Context, cx: &mut Ctx, st: &mut State) {
    let Some((account, rows, info)) = st.recon.as_mut() else { return };
    let mut close = false;
    let mut apply = false;
    let chart = cx.store.chart_of_accounts().unwrap_or_default();
    egui::Window::new(format!("{}  Riconciliazione dell'estratto conto", icon::BANK)).id(egui::Id::new("recon")).collapsible(false).resizable(true).default_size([1040.0, 560.0]).show(ctx, |ui| {
        ui.set_min_width(1000.0);
        ui.horizontal(|ui| {
            ui.label(format!("Conto in prima nota: {}", account));
            ui.label(RichText::new(info.as_str()).color(GRAY));
        });
        ui.label(RichText::new("Verde: già in prima nota. Blu: incasso o pagamento di una scadenza aperta (stesso importo, nome o numero nella causale). Gli altri si registrano con la voce scelta.").small().color(GRAY));
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            egui::Grid::new("recon_grid").striped(true).num_columns(6).spacing([10.0, 4.0]).show(ui, |ui| {
                for h in ["", "Data", "Importo", "Controparte e causale", "Proposta", "Voce"] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for (i, (on, r)) in rows.iter_mut().enumerate() {
                    let recorded = matches!(r.matched, ReconMatch::Recorded(_));
                    ui.add_enabled(!recorded, egui::Checkbox::new(on, ""));
                    ui.label(r.line.date.format("%d/%m/%Y").to_string());
                    ui.label(RichText::new(eur(r.line.amount)).color(if r.line.amount < Decimal::ZERO { RED } else { theme::INK }));
                    ui.add(egui::Label::new(format!("{}  {}", r.line.counterparty, r.line.remittance)).truncate()).on_hover_text(format!("{}\n{}\n{}", r.line.counterparty, r.line.remittance, r.line.reference));
                    match &mut r.matched {
                        ReconMatch::Recorded(_) => {
                            ui.label(RichText::new(format!("già registrato: {}", r.note)).color(GREEN));
                            ui.label("");
                        }
                        ReconMatch::Dues(_) => {
                            ui.label(RichText::new(format!("{}  {}", if r.line.amount > Decimal::ZERO { "Incasso" } else { "Pagamento" }, r.note)).color(theme::tint().main));
                            if ui.small_button("movimento libero").on_hover_text("Non è il pagamento di questa scadenza").clicked() {
                                r.matched = ReconMatch::New(String::new());
                            }
                        }
                        ReconMatch::New(cat) => {
                            ui.label(RichText::new("nuovo movimento").color(ORANGE));
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(cat).desired_width(150.0).hint_text("voce"));
                                let mut c = cat.clone();
                                ui.push_id(("recon_cat", i), |ui| {
                                    ui.menu_button(icon::CARET_DOWN, |ui| {
                                        for a in chart.accounts.iter().filter(|a| matches!(a.kind, AccountKind::Income | AccountKind::Expense)) {
                                            if ui.button(&a.name).clicked() {
                                                c = a.name.clone();
                                                ui.close_menu();
                                            }
                                        }
                                    });
                                });
                                *cat = c;
                            });
                        }
                    }
                    ui.end_row();
                }
            });
        });
        let n = rows.iter().filter(|(on, r)| *on && !matches!(r.matched, ReconMatch::Recorded(_))).count();
        ui.horizontal(|ui| {
            if ui.add_enabled(n > 0, egui::Button::new(format!("Registra {} movimenti", n))).clicked() {
                apply = true;
            }
            if ui.button("Chiudi").clicked() {
                close = true;
            }
        });
    });
    if apply {
        let chosen: Vec<ReconLine> = rows.iter().filter(|(on, _)| *on).map(|(_, r)| r.clone()).collect();
        if let Some(n) = cx.msgs.check(cx.store.apply_reconciliation(account, &chosen), "") {
            cx.msgs.ok(format!("{} movimenti registrati in {}", n, account));
            close = true;
        }
    }
    if close {
        st.recon = None;
    }
}

fn f24_window(ctx: &egui::Context, cx: &mut Ctx, st: &mut State, accounts: &[String]) {
    if st.f24.is_none() {
        return;
    }
    let mut close = false;
    let mut reload = false;
    let mut record = false;
    let mut delete = None;
    let year = st.f24.as_ref().map(|f| f.0).unwrap_or_default();
    let (register, _) = super::memo(ctx, "f24_register", cx.store.generation(), year, || cx.store.f24_payments(year).unwrap_or_default());
    let Some((y, m, items, date, account)) = st.f24.as_mut() else { return };
    egui::Window::new("Versamenti F24").collapsible(false).resizable(true).default_width(720.0).show(ctx, |ui| {
        ui.set_min_width(700.0);
        ui.horizontal(|ui| {
            ui.label("Pagamenti e liquidazione del mese");
            egui::ComboBox::from_id_salt("f24_m").width(110.0).selected_text(month_name(*m)).show_ui(ui, |ui| {
                for i in 1..=12u32 {
                    reload |= ui.selectable_value(m, i, month_name(i)).changed();
                }
            });
            if ui.add(egui::DragValue::new(y).range(2000..=2100).speed(0.1)).changed() {
                reload = true;
            }
        });
        ui.label(RichText::new("Ritenute operate sui compensi pagati nel mese (1040 lavoro autonomo, 1038 provvigioni) e IVA del periodo che si chiude: il commercialista o l'home banking compilano il modello con questi importi.").small().color(GRAY));
        let mut remove = None;
        egui::Grid::new("f24_items").striped(true).num_columns(5).spacing([10.0, 4.0]).show(ui, |ui| {
            for h in ["Codice tributo", "Descrizione", "Rif. (mese)", "Anno", "Importo"] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for (i, it) in items.iter_mut().enumerate() {
                ui.add_sized([60.0, 20.0], egui::TextEdit::singleline(&mut it.code));
                ui.add_sized([240.0, 20.0], egui::TextEdit::singleline(&mut it.label));
                ui.add_sized([50.0, 20.0], egui::TextEdit::singleline(&mut it.period));
                ui.label(it.year.to_string());
                ui.horizontal(|ui| {
                    widgets::dec_field(ui, ("f24_amt", i), &mut it.amount, 90.0, 2);
                    if ui.small_button(icon::TRASH).clicked() {
                        remove = Some(i);
                    }
                });
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            items.remove(i);
        }
        if ui.small_button(format!("{}  Voce", icon::PLUS)).clicked() {
            items.push(F24Item { year: *y, ..Default::default() });
        }
        let total: Decimal = items.iter().map(|i| i.amount).sum();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Totale € {}", eur(total))).strong());
            ui.label("versato il");
            widgets::date_field(ui, "f24_date", date);
            ui.label("dal conto");
            account_combo(ui, "f24_acc", account, accounts);
            if ui.add_enabled(total > Decimal::ZERO, egui::Button::new("Registra versamento")).clicked() {
                record = true;
            }
        });
        ui.separator();
        ui.label(theme::medium(format!("Versamenti registrati nel {}", y)));
        if register.is_empty() {
            ui.label(theme::muted("Nessuno."));
        }
        for p in register.iter() {
            ui.horizontal(|ui| {
                ui.label(p.date.format("%d/%m/%Y").to_string());
                ui.label(format!("€ {}", eur(p.total)));
                ui.label(RichText::new(p.items.iter().map(|i| format!("{} {}", i.code, eur(i.amount))).collect::<Vec<_>>().join(" · ")).color(GRAY));
                ui.label(&p.account);
                if ui.small_button(icon::TRASH).on_hover_text("Annulla il versamento e il movimento di prima nota").clicked() {
                    delete = Some(p.id);
                }
            });
        }
        if ui.button("Chiudi").clicked() {
            close = true;
        }
    });
    if record {
        let (d, a, it) = (*date, account.clone(), items.clone());
        if cx.msgs.check(cx.store.record_f24(d, &a, &it), "Versamento F24 registrato in prima nota").is_some() {
            items.clear();
        }
    }
    if let Some(id) = delete {
        cx.msgs.check(cx.store.delete_f24(id), "Versamento annullato");
    }
    if reload {
        load_f24(cx, st);
    }
    if close {
        st.f24 = None;
    }
}

fn chart_window(ctx: &egui::Context, cx: &mut Ctx, st: &mut State) {
    let Some(c) = st.chart.as_mut() else { return };
    let mut close = false;
    let mut save = false;
    egui::Window::new("Piano dei conti").collapsible(false).resizable(true).default_size([980.0, 560.0]).show(ctx, |ui| {
        ui.set_min_width(940.0);
        ui.label(RichText::new("Conti finanziari (cassa, banche) e voci di entrata e di uscita della prima nota, con il codice del conto nel programma del commercialista per l'esportazione.").color(GRAY));
        let mut remove = None;
        egui::ScrollArea::vertical().max_height(330.0).show(ui, |ui| {
            egui::Grid::new("chart").striped(true).num_columns(7).spacing([8.0, 4.0]).show(ui, |ui| {
                for h in ["Codice", "Nome", "Tipo", "Gruppo", "Codice studio", "IBAN (banche)", ""] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for (i, a) in c.accounts.iter_mut().enumerate() {
                    ui.add_sized([50.0, 20.0], egui::TextEdit::singleline(&mut a.code));
                    ui.add_sized([170.0, 20.0], egui::TextEdit::singleline(&mut a.name));
                    egui::ComboBox::from_id_salt(("chart_kind", i)).width(130.0).selected_text(a.kind.label()).show_ui(ui, |ui| {
                        for k in [AccountKind::Resource, AccountKind::Income, AccountKind::Expense, AccountKind::Technical] {
                            ui.selectable_value(&mut a.kind, k, k.label());
                        }
                    });
                    ui.add_sized([170.0, 20.0], egui::TextEdit::singleline(&mut a.group));
                    ui.add_sized([90.0, 20.0], egui::TextEdit::singleline(&mut a.export_code));
                    ui.add_enabled_ui(a.kind == AccountKind::Resource, |ui| ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut a.iban)));
                    if ui.small_button(icon::TRASH).clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        });
        if let Some(i) = remove {
            c.accounts.remove(i);
        }
        if ui.small_button(format!("{}  Conto", icon::PLUS)).clicked() {
            c.accounts.push(LedgerAccount { kind: AccountKind::Expense, ..Default::default() });
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Conto studio incassi da clienti");
            ui.add(egui::TextEdit::singleline(&mut c.customers_code).desired_width(90.0));
            ui.label("pagamenti a fornitori");
            ui.add(egui::TextEdit::singleline(&mut c.suppliers_code).desired_width(90.0));
        });
        ui.label(theme::medium("Codici IVA dello studio"));
        let vats = cx.store.vat_codes().unwrap_or_default();
        egui::Grid::new("chart_vat").num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
            let mut n = 0;
            for v in vats.iter().filter(|v| v.active) {
                let key = v.natura.clone().filter(|_| v.rate.is_zero()).unwrap_or_else(|| v.rate.normalize().to_string());
                if c.vat_codes.iter().filter(|(k, _)| *k == key).count() == 0 {
                    c.vat_codes.push((key.clone(), String::new()));
                }
                if let Some(slot) = c.vat_codes.iter_mut().find(|(k, _)| *k == key) {
                    ui.label(format!("{}  {}", v.code, widgets::truncate(&v.description, 28)));
                    ui.add(egui::TextEdit::singleline(&mut slot.1).desired_width(70.0).hint_text(key.as_str()));
                    n += 1;
                    if n % 2 == 0 {
                        ui.end_row();
                    }
                }
            }
        });
        ui.horizontal(|ui| {
            if theme::primary(ui, icon::CHECK, "Salva").clicked() {
                save = true;
            }
            if ui.button("Chiudi").clicked() {
                close = true;
            }
        });
    });
    if save {
        let mut c = c.clone();
        c.vat_codes.retain(|(_, v)| !v.trim().is_empty());
        if cx.msgs.check(cx.store.set_chart_of_accounts(&c), "Piano dei conti salvato").is_some() {
            close = true;
        }
    }
    if close {
        st.chart = None;
    }
}

fn export_window(ctx: &egui::Context, cx: &mut Ctx, st: &mut State) {
    let Some((from, to)) = st.export.as_mut() else { return };
    let mut close = false;
    egui::Window::new("Esportazione per il commercialista").collapsible(false).resizable(false).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Dal");
            widgets::date_field(ui, "exp_from", from);
            ui.label("al");
            widgets::date_field(ui, "exp_to", to);
        });
        ui.label(RichText::new("File CSV (separatore punto e virgola) con i codici del piano dei conti: lo studio li importa con il tracciato libero del suo programma.").small().color(GRAY));
        ui.horizontal(|ui| {
            let save = |name: &str, data: Result<String, openfatt_store::StoreError>, cx: &mut Ctx| {
                match data {
                    Ok(text) => {
                        if let Some(p) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name(name).save_file() {
                            cx.msgs.check(std::fs::write(&p, text.as_bytes()), "File esportato");
                        }
                    }
                    Err(e) => cx.msgs.err(e.to_string()),
                }
            };
            if ui.button("Prima nota").clicked() {
                save(&format!("prima_nota_{}_{}.csv", from.format("%Y%m%d"), to.format("%Y%m%d")), cx.store.ledger_export_csv(*from, *to), cx);
            }
            if ui.button("Registri IVA").clicked() {
                save(&format!("registri_iva_{}_{}.csv", from.format("%Y%m%d"), to.format("%Y%m%d")), cx.store.vat_export_csv(*from, *to), cx);
            }
            if ui.button("Chiudi").clicked() {
                close = true;
            }
        });
        ui.separator();
        ui.label(theme::medium("Entrate e uscite per voce nel periodo"));
        let (rows, _) = super::memo(ui.ctx(), "category_totals", cx.store.generation(), (*from, *to), || cx.store.category_totals(*from, *to).unwrap_or_default());
        egui::Grid::new("cat_tot").striped(true).num_columns(4).spacing([16.0, 3.0]).show(ui, |ui| {
            for h in ["Voce", "Gruppo", "Entrate", "Uscite"] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for r in rows.iter() {
                ui.label(&r.category);
                ui.label(RichText::new(&r.group).color(GRAY));
                ui.label(eur(r.income));
                ui.label(eur(r.expense));
                ui.end_row();
            }
        });
    });
    if close {
        st.export = None;
    }
}
