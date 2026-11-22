//! Registrazione di incassi e pagamenti: un importo ricevuto (o pagato) con data, modalità,
//! conto e riferimento si ripartisce su una o più scadenze dello stesso cliente o fornitore.
//! Una quota minore del residuo è un acconto: la scadenza resta aperta per la differenza, che
//! si può anche indicare a mano ("resterà da saldare") o chiudere con un abbuono.

use chrono::NaiveDate;
use eframe::egui::{self, RichText};
use openfatt_core::codes;
use openfatt_store::{default_account, distribute, Allocation, DueRow, NewPayment, PaymentRow, Store, COMMON_METHODS};
use rust_decimal::Decimal;

use super::theme::{self, icon, Tone};
use super::widgets::{self, eur};
use super::{Ctx, Nav};

/// Una scadenza nella finestra di registrazione.
pub struct PayLine {
    pub due: DueRow,
    /// Quota registrata ora.
    pub amount: Decimal,
    /// Quanto resterà da saldare (modificabile: la quota si ricalcola).
    pub left: Decimal,
    pub writeoff: bool,
}

pub struct PayForm {
    pub receivable: bool,
    pub party: Option<(Option<i64>, String)>,
    pub date: NaiveDate,
    pub mp_code: String,
    pub account: String,
    pub reference: String,
    pub notes: String,
    /// Importo ricevuto o pagato: modificandolo si ripartisce dalla scadenza più vecchia.
    pub received: Decimal,
    pub lines: Vec<PayLine>,
}

impl PayForm {
    /// Finestra per un cliente/fornitore: tutte le sue scadenze aperte, con quelle indicate
    /// già spuntate per l'intero residuo.
    pub fn for_party(store: &Store, party_id: Option<i64>, party_name: &str, receivable: bool, preselect: &[i64]) -> Self {
        let mut f = PayForm::empty(receivable);
        f.load_party(store, party_id, party_name, preselect);
        f
    }

    /// Acconto su scadenze precise (di solito quelle di un documento): stanno in cima e
    /// l'importo che si scrive va prima a loro; nessuna quota già proposta.
    pub fn deposit(store: &Store, party_id: Option<i64>, party_name: &str, receivable: bool, dues: &[i64]) -> Self {
        let mut f = PayForm::for_party(store, party_id, party_name, receivable, dues);
        for l in f.lines.iter_mut() {
            l.amount = Decimal::ZERO;
            l.left = l.due.residual();
        }
        f.received = Decimal::ZERO;
        f
    }

    /// Finestra senza cliente: si sceglie nella finestra stessa.
    pub fn empty(receivable: bool) -> Self {
        PayForm {
            receivable,
            party: None,
            date: chrono::Local::now().date_naive(),
            mp_code: "MP05".into(),
            account: "Banca".into(),
            reference: String::new(),
            notes: String::new(),
            received: Decimal::ZERO,
            lines: Vec::new(),
        }
    }

    fn load_party(&mut self, store: &Store, party_id: Option<i64>, party_name: &str, preselect: &[i64]) {
        let dues = store.open_dues_of_party(party_id, party_name, self.receivable).unwrap_or_default();
        // modalità proposta: quella della prima scadenza scelta (contanti, bonifico...)
        if let Some(d) = dues.iter().find(|d| preselect.contains(&d.id)).or(dues.first()) {
            if !d.mp_code.is_empty() {
                self.mp_code = d.mp_code.clone();
                self.account = default_account(&d.mp_code).to_string();
            }
        }
        // le scadenze scelte in cima: l'importo ricevuto si ripartisce prima su di loro
        let (mut chosen, others): (Vec<DueRow>, Vec<DueRow>) = dues.into_iter().partition(|d| preselect.contains(&d.id));
        chosen.extend(others);
        self.lines = chosen
            .into_iter()
            .map(|due| {
                let amount = if preselect.contains(&due.id) { due.residual() } else { Decimal::ZERO };
                PayLine { left: due.residual() - amount, amount, due, writeoff: false }
            })
            .collect();
        self.party = Some((party_id, party_name.to_string()));
        self.received = self.total();
    }

    pub fn total(&self) -> Decimal {
        self.lines.iter().map(|l| l.amount).sum()
    }

    fn open_total(&self) -> Decimal {
        self.lines.iter().map(|l| l.due.residual()).sum()
    }

    /// Ripartisce l'importo ricevuto dalla scadenza più vecchia.
    fn spread(&mut self) {
        let parts = distribute(self.received, &self.lines.iter().map(|l| l.due.residual()).collect::<Vec<_>>());
        for (l, a) in self.lines.iter_mut().zip(parts) {
            l.amount = a;
            l.left = l.due.residual() - a;
            l.writeoff = false;
        }
    }

    fn set_amount(&mut self, i: usize) {
        let l = &mut self.lines[i];
        l.amount = l.amount.clamp(Decimal::ZERO, l.due.residual());
        l.left = l.due.residual() - l.amount;
        if l.left.is_zero() {
            l.writeoff = false;
        }
        self.received = self.total();
    }

    fn set_left(&mut self, i: usize) {
        let l = &mut self.lines[i];
        l.left = l.left.clamp(Decimal::ZERO, l.due.residual());
        l.amount = l.due.residual() - l.left;
        if l.left.is_zero() {
            l.writeoff = false;
        }
        self.received = self.total();
    }

    fn request(&self) -> NewPayment {
        NewPayment {
            date: self.date,
            mp_code: self.mp_code.clone(),
            account: self.account.clone(),
            reference: self.reference.clone(),
            notes: self.notes.clone(),
            allocations: self.lines.iter().filter(|l| l.amount > Decimal::ZERO || l.writeoff).map(|l| Allocation { due_id: l.due.id, amount: l.amount, writeoff: l.writeoff }).collect(),
        }
    }
}

fn reference_hint(mp: &str) -> &'static str {
    match mp {
        "MP02" | "MP03" => "Numero dell'assegno e banca",
        "MP05" => "CRO / TRN del bonifico",
        "MP08" => "Carta o numero di autorizzazione",
        "MP12" => "Numero della ricevuta bancaria",
        _ => "Riferimento (facoltativo)",
    }
}

/// Elenco a tendina delle modalità di pagamento: prima le più usate.
pub fn method_combo(ui: &mut egui::Ui, id: &str, mp: &mut String, account: &mut String) {
    let before = mp.clone();
    let label = if mp.is_empty() { "Non indicata".to_string() } else { codes::describe(codes::MODALITA_PAGAMENTO, mp).to_string() };
    egui::ComboBox::from_id_salt(id).selected_text(label).width(200.0).height(420.0).show_ui(ui, |ui| {
        for c in COMMON_METHODS {
            ui.selectable_value(mp, c.to_string(), codes::describe(codes::MODALITA_PAGAMENTO, c));
        }
        ui.separator();
        for (c, t) in codes::MODALITA_PAGAMENTO.iter().filter(|(c, _)| !COMMON_METHODS.contains(c)) {
            ui.selectable_value(mp, c.to_string(), RichText::new(*t).color(theme::MUTED));
        }
    });
    // il conto segue la modalità finché non è stato cambiato a mano
    if *mp != before && account.as_str() == default_account(&before) {
        *account = default_account(mp).to_string();
    }
}

/// Finestra di registrazione. Restituisce true quando va chiusa.
pub fn form_window(ctx: &egui::Context, cx: &mut Ctx, f: &mut PayForm, accounts: &[String], parties: &[(Option<i64>, String)]) -> bool {
    let mut close = false;
    let verb = if f.receivable { "incasso" } else { "pagamento" };
    let title = format!("{}  Registra {}", icon::HAND_COINS, verb);
    egui::Window::new(title).id(egui::Id::new("pay_form")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).default_width(860.0).show(ctx, |ui| {
        ui.set_width(860.0);
        // cliente o fornitore
        ui.horizontal(|ui| {
            ui.label(theme::muted(if f.receivable { "Cliente" } else { "Fornitore" }));
            let current = f.party.as_ref().map(|p| p.1.clone()).unwrap_or_else(|| "Scegliere...".into());
            let mut chosen = None;
            egui::ComboBox::from_id_salt("pay_party").selected_text(theme::medium(current)).width(360.0).height(400.0).show_ui(ui, |ui| {
                for p in parties {
                    if ui.selectable_label(f.party.as_ref() == Some(p), &p.1).clicked() {
                        chosen = Some(p.clone());
                    }
                }
            });
            if let Some((id, name)) = chosen {
                f.load_party(cx.store, id, &name, &[]);
            }
            if f.party.is_some() {
                ui.label(theme::muted(format!("{} scadenze aperte · da saldare € {}", f.lines.len(), eur(f.open_total()))));
            }
        });
        ui.add_space(8.0);
        egui::Grid::new("pay_head").num_columns(4).spacing([14.0, 8.0]).show(ui, |ui| {
            ui.label("Data");
            widgets::date_field(ui, "pay_date", &mut f.date);
            ui.label("Modalità");
            method_combo(ui, "pay_mp", &mut f.mp_code, &mut f.account);
            ui.end_row();
            ui.label("Conto");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("pay_acc").selected_text(f.account.clone()).width(120.0).show_ui(ui, |ui| {
                    for a in accounts {
                        ui.selectable_value(&mut f.account, a.clone(), a);
                    }
                });
                ui.add(egui::TextEdit::singleline(&mut f.account).desired_width(110.0).hint_text("o nuovo conto"));
            });
            ui.label("Riferimento");
            ui.add(egui::TextEdit::singleline(&mut f.reference).desired_width(260.0).hint_text(reference_hint(&f.mp_code)));
            ui.end_row();
            ui.label(theme::medium(if f.receivable { "Importo ricevuto" } else { "Importo pagato" }));
            ui.horizontal(|ui| {
                if widgets::dec_field(ui, "pay_received", &mut f.received, 120.0, 2) {
                    f.spread();
                }
                if theme::ghost(ui, !f.lines.is_empty(), icon::CHECKS, "Tutto").on_hover_text("Salda tutte le scadenze aperte").clicked() {
                    f.received = f.open_total();
                    f.spread();
                }
            });
            ui.label("");
            ui.label(theme::muted("L'importo va prima alle scadenze in cima (quelle scelte, poi le più vecchie); si corregge riga per riga."));
            ui.end_row();
        });
        ui.add_space(10.0);
        if f.party.is_none() {
            ui.label(theme::muted(format!("Scegliere il {} per vedere le scadenze da saldare.", if f.receivable { "cliente" } else { "fornitore" })));
        } else if f.lines.is_empty() {
            ui.label(theme::muted("Nessuna scadenza da saldare."));
        } else {
            lines_table(ui, f);
        }
        ui.add_space(10.0);
        theme::rule(ui);
        ui.add_space(8.0);
        // riepilogo: cosa si registra e cosa resta aperto
        let total = f.total();
        let touched: Vec<&PayLine> = f.lines.iter().filter(|l| l.amount > Decimal::ZERO || l.writeoff).collect();
        let partial: Vec<&PayLine> = touched.iter().copied().filter(|l| l.left > Decimal::ZERO && !l.writeoff).collect();
        ui.horizontal_wrapped(|ui| {
            ui.label(theme::bold(format!("€ {}", eur(total))).size(18.0).color(theme::tint().main));
            ui.label(theme::muted(format!(
                "su {} {}",
                touched.len(),
                if touched.len() == 1 { "scadenza" } else { "scadenze" }
            )));
            let saldate = touched.iter().filter(|l| l.left.is_zero() || l.writeoff).count();
            if saldate > 0 {
                theme::pill(ui, &format!("{} saldate", saldate), Tone::Positive);
            }
            for l in &partial {
                theme::pill(ui, &format!("acconto: su {} restano € {}", short_label(&l.due.doc_label), eur(l.left)), Tone::Warning);
            }
        });
        ui.add_space(6.0);
        ui.add(egui::TextEdit::singleline(&mut f.notes).desired_width(f32::INFINITY).hint_text("Note (facoltative)"));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let label = format!("Registra {} di € {}", verb, eur(total));
            if theme::primary_enabled(ui, total > Decimal::ZERO, icon::CHECK, &label).clicked() {
                let n = touched.len();
                if cx.msgs.check(cx.store.register_payment(&f.request()), &format!("{} di € {} registrato su {} {}", if f.receivable { "Incasso" } else { "Pagamento" }, eur(total), n, if n == 1 { "scadenza" } else { "scadenze" })).is_some() {
                    close = true;
                }
            }
            if theme::button(ui, icon::X, "Annulla").clicked() {
                close = true;
            }
        });
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    close
}

/// "Fattura 3 del 10/01/2026" -> "Fattura 3".
fn short_label(l: &str) -> &str {
    l.split(" del ").next().unwrap_or(l)
}

fn lines_table(ui: &mut egui::Ui, f: &mut PayForm) {
    let today = chrono::Local::now().date_naive();
    let mut changed_amount = None;
    let mut changed_left = None;
    egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("pay_lines").num_columns(7).striped(true).spacing([12.0, 6.0]).min_col_width(20.0).show(ui, |ui| {
            for t in ["", "Documento", "Scadenza", "Residuo", if f.receivable { "Incassato ora" } else { "Pagato ora" }, "Resterà da saldare", "Abbuono"] {
                ui.label(RichText::new(t).size(12.0).color(theme::MUTED));
            }
            ui.end_row();
            for (i, l) in f.lines.iter_mut().enumerate() {
                let mut on = l.amount > Decimal::ZERO || l.writeoff;
                if ui.checkbox(&mut on, "").changed() {
                    l.amount = if on { l.due.residual() } else { Decimal::ZERO };
                    l.left = l.due.residual() - l.amount;
                    l.writeoff = false;
                    changed_amount = Some(i);
                }
                ui.label(if on { theme::medium(&l.due.doc_label) } else { RichText::new(&l.due.doc_label) });
                let late = l.due.date < today;
                ui.label(RichText::new(l.due.date.format("%d/%m/%Y").to_string()).color(if late { theme::NEGATIVE } else { theme::INK }));
                ui.horizontal(|ui| {
                    ui.label(format!("€ {}", eur(l.due.residual())));
                    if !l.due.paid.is_zero() {
                        ui.label(RichText::new(format!("(già pagati € {})", eur(l.due.paid))).size(11.5).color(theme::MUTED));
                    }
                });
                if widgets::dec_field(ui, ("pay_amount", l.due.id), &mut l.amount, 100.0, 2) {
                    changed_amount = Some(i);
                }
                if widgets::dec_field(ui, ("pay_left", l.due.id), &mut l.left, 100.0, 2) {
                    changed_left = Some(i);
                }
                let can = l.left > Decimal::ZERO && l.amount > Decimal::ZERO;
                ui.add_enabled_ui(can || l.writeoff, |ui| {
                    ui.checkbox(&mut l.writeoff, "").on_hover_text(format!("Chiude la scadenza: i € {} che restano diventano abbuono", eur(l.left)));
                });
                ui.end_row();
            }
        });
    });
    if let Some(i) = changed_amount {
        f.set_amount(i);
    }
    if let Some(i) = changed_left {
        f.set_left(i);
    }
}

/// Storico dei pagamenti di un documento, con annullamento e spostamento del residuo.
pub struct History {
    pub due: DueRow,
    new_date: NaiveDate,
    confirm: Option<i64>,
    confirm_all: bool,
}

impl History {
    pub fn new(due: DueRow) -> Self {
        let new_date = due.date.max(chrono::Local::now().date_naive()) + chrono::Duration::days(30);
        History { due, new_date, confirm: None, confirm_all: false }
    }
}

/// Finestra dello storico. Restituisce true quando va chiusa.
pub fn history_window(ctx: &egui::Context, cx: &mut Ctx, h: &mut History) -> bool {
    let mut close = false;
    let gen = cx.store.generation();
    let doc_id = h.due.document_id;
    let (payments, _) = super::memo(ctx, "doc_payments", gen, doc_id, || cx.store.payments_of_document(doc_id).unwrap_or_default());
    // la scadenza come è adesso
    let (current, _) = super::memo(ctx, "history_due", gen, h.due.id, || cx.store.dues_by_id(&[h.due.id]).unwrap_or_default());
    if let Some(d) = current.first() {
        h.due = d.clone();
    }
    let d = h.due.clone();
    egui::Window::new(format!("{}  Pagamenti di {}", icon::CLOCK_COUNTER_CLOCKWISE, short_label(&d.doc_label)))
        .id(egui::Id::new("pay_history"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(720.0);
            ui.horizontal(|ui| {
                if ui.link(theme::medium(&d.doc_label)).clicked() {
                    cx.nav.push(Nav::OpenDocument(d.document_id));
                    close = true;
                }
                ui.label(theme::muted(format!("· {}", d.party_name)));
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(format!("Scadenza del {}: € {}", d.date.format("%d/%m/%Y"), eur(d.amount)));
                ui.label(theme::muted(format!("· pagati € {}", eur(d.paid))));
                if !d.writeoff.is_zero() {
                    ui.label(theme::muted(format!("· abbuono € {}", eur(d.writeoff))));
                }
                let r = d.residual();
                if r > Decimal::ZERO {
                    theme::pill(ui, &format!("da saldare € {}", eur(r)), if d.paid.is_zero() { Tone::Neutral } else { Tone::Warning });
                } else {
                    theme::pill(ui, "saldata", Tone::Positive);
                }
            });
            ui.add_space(10.0);
            ui.label(theme::medium("Registrazioni sul documento"));
            ui.add_space(4.0);
            if payments.is_empty() {
                ui.label(theme::muted(if d.paid.is_zero() { "Nessun incasso o pagamento registrato." } else { "Pagamenti registrati prima di questa versione o importati: si possono solo annullare tutti insieme." }));
            }
            let mut cancel = None;
            for p in payments.iter() {
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(theme::medium(p.date.format("%d/%m/%Y").to_string()));
                        ui.label(p.method_text());
                        ui.label(theme::muted(format!("· {}", p.account)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::ghost_colored(ui, true, icon::ARROW_COUNTER_CLOCKWISE, "Annulla", theme::NEGATIVE).clicked() {
                                cancel = Some(p.id);
                            }
                            let here: Decimal = p.lines.iter().filter(|l| l.document_id == doc_id).map(|l| l.amount).sum();
                            ui.label(theme::bold(format!("€ {}", eur(here))));
                        });
                    });
                    let others: Vec<String> = p.lines.iter().filter(|l| l.document_id != doc_id).map(|l| format!("{} € {}", short_label(&l.doc_label), eur(l.amount))).collect();
                    if !others.is_empty() {
                        ui.label(theme::muted(format!("Registrazione di € {} che copre anche: {}", eur(p.total), others.join(", "))).size(12.5));
                    }
                    if !p.notes.is_empty() {
                        ui.label(theme::muted(&p.notes).size(12.5));
                    }
                });
                ui.add_space(4.0);
            }
            if let Some(id) = cancel {
                h.confirm = Some(id);
            }
            if payments.is_empty() && !d.paid.is_zero() && theme::ghost_colored(ui, true, icon::ARROW_COUNTER_CLOCKWISE, "Annulla tutti i pagamenti della scadenza", theme::NEGATIVE).clicked() {
                h.confirm_all = true;
            }
            if d.residual() > Decimal::ZERO {
                ui.add_space(10.0);
                ui.label(theme::medium("Sposta quello che resta da saldare"));
                ui.horizontal(|ui| {
                    ui.label(format!("€ {} al", eur(d.residual())));
                    widgets::date_field(ui, "resched_date", &mut h.new_date);
                    if theme::button(ui, icon::CALENDAR_PLUS, "Sposta").clicked() {
                        cx.msgs.check(cx.store.reschedule_residual(d.id, h.new_date), "Residuo spostato a una nuova scadenza");
                    }
                });
                ui.label(theme::muted(if d.paid.is_zero() { "Cambia la data della scadenza (se il documento viene modificato e salvato di nuovo, le scadenze si ricalcolano dal pagamento scelto)." } else { "La parte già pagata resta alla data originale; il residuo diventa una nuova scadenza." }).size(12.5));
            }
            ui.add_space(10.0);
            if theme::button(ui, icon::X, "Chiudi").clicked() {
                close = true;
            }
        });
    if let Some(id) = h.confirm {
        let p = payments.iter().find(|p| p.id == id);
        let text = match p {
            Some(p) if p.lines.len() > 1 => format!(
                "Annullare la registrazione del {} di € {}? Copre {} scadenze: tornano tutte da saldare per la loro quota e i movimenti di prima nota vengono eliminati.",
                p.date.format("%d/%m/%Y"),
                eur(p.total),
                p.lines.len()
            ),
            Some(p) => format!("Annullare la registrazione del {} di € {}? Il movimento di prima nota viene eliminato.", p.date.format("%d/%m/%Y"), eur(p.total)),
            None => "Annullare la registrazione?".into(),
        };
        match widgets::confirm(ctx, "Annulla registrazione", &text) {
            Some(true) => {
                cx.msgs.check(cx.store.cancel_payment(id), "Registrazione annullata");
                h.confirm = None;
            }
            Some(false) => h.confirm = None,
            None => {}
        }
    }
    if h.confirm_all {
        match widgets::confirm(ctx, "Annulla pagamenti", "Annullare tutti i pagamenti registrati su questa scadenza (e i movimenti di prima nota collegati)?") {
            Some(true) => {
                cx.msgs.check(cx.store.unpay_due(d.id), "Pagamenti annullati");
                h.confirm_all = false;
            }
            Some(false) => h.confirm_all = false,
            None => {}
        }
    }
    if h.confirm.is_none() && !h.confirm_all && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    close
}

/// Testo breve dei documenti coperti da una registrazione.
pub fn docs_text(p: &PaymentRow) -> String {
    p.lines.iter().map(|l| format!("{} ({})", short_label(&l.doc_label), eur(l.amount))).collect::<Vec<_>>().join(", ")
}
