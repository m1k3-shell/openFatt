//! Finestra di composizione delle email e impostazioni della posta.

use std::sync::mpsc::Sender;

use eframe::egui::{self, RichText};
use openfatt_core::mailtext::{self, MailTemplate, MailTemplates};
use openfatt_pec::mail::{self, MailAttachment, MailConfig, MailMode};
use openfatt_pec::TlsMode;
use openfatt_store::{MailStatus, Store};
use rust_decimal::Decimal;

use super::theme::{self, icon, Tone};
use super::widgets;
use super::{Ctx, Msgs, Nav};
use crate::mailer::{self, Compose, Job, MailJobResult};
use crate::services;

/// Coda dei messaggi da rivedere e inviare.
#[derive(Default)]
pub struct Composer {
    pub queue: Vec<Compose>,
    password: String,
    remember: bool,
    /// Messaggi affidati al thread di invio e non ancora conclusi.
    pub in_flight: usize,
    pub sent: usize,
    pub failed: usize,
}

impl Composer {
    pub fn open(&mut self, list: Vec<Compose>) {
        self.queue = list;
        self.password.clear();
        self.remember = true;
    }
    pub fn busy_text(&self) -> Option<String> {
        (self.in_flight > 0).then(|| format!("Invio email in corso ({} in coda)...", self.in_flight))
    }

    /// Esito di un messaggio dal thread di invio.
    pub fn on_result(&mut self, store: &Store, msgs: &mut Msgs, r: MailJobResult) {
        match r {
            MailJobResult::Done { mut log, result } => {
                self.in_flight = self.in_flight.saturating_sub(1);
                match result {
                    Ok(note) => {
                        log.status = MailStatus::Sent;
                        log.detail = note;
                        self.sent += 1;
                    }
                    Err(e) => {
                        log.status = MailStatus::Failed;
                        log.detail = e.clone();
                        self.failed += 1;
                        msgs.err(format!("Email a {} non inviata: {}", log.recipients, e));
                    }
                }
                if let Err(e) = store.log_mail(&log) {
                    msgs.err(e.to_string());
                }
            }
            MailJobResult::Finished => {
                if self.in_flight == 0 {
                    match (self.sent, self.failed) {
                        (0, _) => {}
                        (1, 0) => msgs.ok("Email inviata"),
                        (n, 0) => msgs.ok(format!("{} email inviate", n)),
                        (n, f) => msgs.warn(format!("{} email inviate, {} non inviate (vedi registro)", n, f)),
                    }
                    self.sent = 0;
                    self.failed = 0;
                }
            }
            MailJobResult::Tested(r) => match r {
                Ok(s) => msgs.ok(s),
                Err(e) => msgs.err(e),
            },
        }
    }

    /// Invia (o apre nel programma di posta) i primi `n` messaggi in coda.
    /// I messaggi che non si possono inviare restano in coda con l'errore mostrato.
    fn send(&mut self, store: &Store, msgs: &mut Msgs, tx: &Sender<MailJobResult>, n: usize) {
        let cfg = mailer::mail_config(store);
        let n = n.min(self.queue.len());
        for (i, c) in self.queue.iter().take(n).enumerate() {
            if let Err(e) = c.outgoing() {
                msgs.err(if n > 1 { format!("Messaggio {} ({}): {}", i + 1, c.party_name, e) } else { e });
                return;
            }
        }
        let batch: Vec<Compose> = self.queue.drain(..n).collect();
        let mut jobs = Vec::new();
        let mut back = Vec::new();
        let mut opened = 0;
        let mut error: Option<String> = None;
        let mut remembered = false;
        for c in batch {
            if error.is_some() {
                back.push(c);
                continue;
            }
            if cfg.mode == MailMode::MailClient && !c.via_pec {
                match mailer::open_in_client(store, &c) {
                    Ok(_) => {
                        let mut log = c.log_entry();
                        log.status = MailStatus::Prepared;
                        log.detail = "aperto nel programma di posta".into();
                        let _ = store.log_mail(&log);
                        opened += 1;
                    }
                    Err(e) => {
                        error = Some(e);
                        back.push(c);
                    }
                }
                continue;
            }
            let pec_account = c.via_pec || cfg.mode == MailMode::Pec;
            let (acc, stored) = match mailer::account_for(store, pec_account) {
                Ok(x) => x,
                Err(e) => {
                    error = Some(e);
                    back.push(c);
                    continue;
                }
            };
            let pw = match stored.filter(|p| !p.is_empty()) {
                Some(p) => p,
                None if !self.password.is_empty() => {
                    if self.remember && !remembered {
                        remembered = true;
                        let r = if pec_account { crate::config::store_password(&services::pec_config(store).address, &self.password) } else { mailer::store_password(&acc.address, &self.password) };
                        if let Err(e) = r {
                            msgs.warn(format!("Password non memorizzata: {}", e));
                        }
                    }
                    self.password.clone()
                }
                None => {
                    error = Some(format!("Inserire la password della casella {}", acc.address));
                    back.push(c);
                    continue;
                }
            };
            let mail = c.outgoing().expect("controllato sopra");
            jobs.push(Job { acc, password: pw, reply_to: cfg.reply_to.clone(), bcc_self: cfg.bcc_self, mail, log: c.log_entry() });
        }
        if !jobs.is_empty() {
            self.in_flight += jobs.len();
            mailer::spawn(jobs, tx.clone());
        }
        if opened > 0 {
            msgs.ok(if opened == 1 { "Messaggio aperto nel programma di posta: controllarlo e premere Invia".to_string() } else { format!("{} messaggi aperti nel programma di posta", opened) });
        }
        if let Some(e) = error {
            msgs.err(e);
        }
        for c in back.into_iter().rev() {
            self.queue.insert(0, c);
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, store: &Store, msgs: &mut Msgs, tx: &Sender<MailJobResult>) {
        if self.queue.is_empty() {
            return;
        }
        let cfg = mailer::mail_config(store);
        let total = self.queue.len();
        let mut action: Option<usize> = None; // quanti messaggi inviare
        let mut skip = false;
        let mut close = false;
        let pec_ready = services::pec_config(store).validate().is_ok();
        let c = &mut self.queue[0];
        let pec_account = c.via_pec || cfg.mode == MailMode::Pec;
        let uses_server = cfg.mode != MailMode::MailClient || c.via_pec;
        let needs_pw = uses_server && matches!(mailer::account_for(store, pec_account), Ok((_, ref pw)) if pw.as_ref().is_none_or(|p| p.is_empty()));
        let account_err = mailer::account_for(store, pec_account).err();
        egui::Window::new(format!("{}  Nuova email", icon::ENVELOPE_SIMPLE))
            .id(egui::Id::new("composer"))
            .collapsible(false)
            .resizable(true)
            .default_width(720.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(theme::medium(&c.party_name).size(16.0));
                    if total > 1 {
                        theme::pill(ui, &format!("1 di {}", total), Tone::Neutral);
                    }
                    let tone = match (c.kind, c.level) {
                        (openfatt_store::MailKind::Reminder, 3) => Tone::Negative,
                        (openfatt_store::MailKind::Reminder, _) => Tone::Warning,
                        _ => Tone::Neutral,
                    };
                    theme::pill(ui, &c.title(), tone);
                });
                if !c.note.is_empty() {
                    ui.label(RichText::new(format!("{}  {}", icon::INFO, c.note)).color(theme::WARNING));
                }
                if let Some(e) = &account_err {
                    ui.label(RichText::new(format!("{}  {}", icon::WARNING_CIRCLE, e)).color(theme::NEGATIVE));
                }
                ui.add_space(4.0);
                let w = ui.available_width();
                egui::Grid::new("compose").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                    ui.label(theme::muted("A"));
                    let bad = mail::parse_addresses(&c.to).is_err() || c.to.trim().is_empty();
                    ui.add(egui::TextEdit::singleline(&mut c.to).desired_width(w - 90.0).text_color(if bad { theme::NEGATIVE } else { theme::INK }).hint_text("indirizzo@cliente.it; altro@cliente.it"));
                    ui.end_row();
                    ui.label(theme::muted("Cc"));
                    ui.add(egui::TextEdit::singleline(&mut c.cc).desired_width(w - 90.0));
                    ui.end_row();
                    ui.label(theme::muted("Oggetto"));
                    ui.add(egui::TextEdit::singleline(&mut c.subject).desired_width(w - 90.0));
                    ui.end_row();
                    ui.label(theme::muted("Invia da"));
                    ui.horizontal(|ui| {
                        let from = match (c.via_pec, cfg.mode) {
                            (true, _) | (_, MailMode::Pec) => format!("PEC {}", services::pec_config(store).address),
                            (_, MailMode::MailClient) => format!("programma di posta ({})", cfg.address),
                            _ => cfg.address.clone(),
                        };
                        ui.label(from);
                        if pec_ready && cfg.mode != MailMode::Pec {
                            ui.checkbox(&mut c.via_pec, "usa la casella PEC").on_hover_text("La PEC dà prova legale di invio e consegna (utile per l'ultimo avviso di pagamento)");
                        }
                    });
                    ui.end_row();
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical().id_salt("compose_body").max_height(300.0).show(ui, |ui| {
                    ui.add(egui::TextEdit::multiline(&mut c.body).desired_width(f32::INFINITY).desired_rows(14).margin(egui::Margin::symmetric(10, 8)));
                });
                ui.add_space(6.0);
                // allegati
                let mut remove = None;
                ui.horizontal_wrapped(|ui| {
                    ui.label(theme::muted("Allegati"));
                    for (i, a) in c.attachments.iter().enumerate() {
                        egui::Frame::new().fill(theme::RAISED).stroke(egui::Stroke::new(1.0_f32, theme::BORDER)).corner_radius(6.0).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                let ic = if a.mime == "application/pdf" { icon::FILE_PDF } else { icon::PAPERCLIP };
                                if ui.add(egui::Label::new(format!("{} {}", ic, a.name)).sense(egui::Sense::click())).on_hover_text("Apri").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    if let Err(e) = services::open_bytes(&a.name, &a.data) {
                                        msgs.err(e);
                                    }
                                }
                                ui.label(theme::muted(size_label(a.data.len())).size(12.0));
                                if ui.small_button(icon::X).on_hover_text("Togli l'allegato").clicked() {
                                    remove = Some(i);
                                }
                            });
                        });
                    }
                    if theme::ghost(ui, true, icon::PAPERCLIP, "Aggiungi file").clicked() {
                        if let Some(paths) = rfd::FileDialog::new().pick_files() {
                            for p in paths {
                                match std::fs::read(&p) {
                                    Ok(data) => {
                                        let name = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "allegato".into());
                                        let mime = match p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
                                            Some("pdf") => "application/pdf",
                                            Some("xml") => "application/xml",
                                            Some("jpg") | Some("jpeg") => "image/jpeg",
                                            Some("png") => "image/png",
                                            Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                                            Some("csv") | Some("txt") => "text/plain",
                                            _ => "application/octet-stream",
                                        };
                                        c.attachments.push(MailAttachment { name, mime: mime.into(), data });
                                    }
                                    Err(e) => msgs.err(format!("{}: {}", p.display(), e)),
                                }
                            }
                        }
                    }
                });
                if let Some(i) = remove {
                    c.attachments.remove(i);
                }
                let size: usize = c.attachments.iter().map(|a| a.data.len()).sum();
                if size > mail::MAX_MAIL_BYTES {
                    ui.label(RichText::new("Gli allegati superano i 20 MB: molti server rifiuterebbero il messaggio.").color(theme::NEGATIVE));
                }
                if needs_pw {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(theme::muted("Password della casella"));
                        ui.add(egui::TextEdit::singleline(&mut self.password).password(true).desired_width(200.0));
                        ui.checkbox(&mut self.remember, "ricorda nel Gestore credenziali di Windows");
                    });
                }
                ui.add_space(8.0);
                theme::rule(ui);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if theme::button(ui, icon::X, if total > 1 { "Annulla tutti" } else { "Annulla" }).clicked() {
                        close = true;
                    }
                    if total > 1 && theme::ghost(ui, true, icon::SKIP_FORWARD, "Salta questo").clicked() {
                        skip = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let client = cfg.mode == MailMode::MailClient && !c.via_pec;
                        let ok = account_err.is_none();
                        let (ic, label) = if client { (icon::ARROW_SQUARE_OUT, "Apri nel programma di posta") } else { (icon::PAPER_PLANE_TILT, "Invia") };
                        if theme::primary_enabled(ui, ok, ic, label).clicked() {
                            action = Some(1);
                        }
                        if total > 1 && theme::button_enabled(ui, ok, icon::PAPER_PLANE_TILT, &format!("Invia tutti ({})", total)).on_hover_text("Invia tutti i messaggi in coda così come sono stati preparati").clicked() {
                            action = Some(total);
                        }
                    });
                });
            });
        if close {
            self.queue.clear();
        } else if skip {
            self.queue.remove(0);
        } else if let Some(n) = action {
            self.send(store, msgs, tx, n);
        }
    }
}

fn size_label(n: usize) -> String {
    if n < 1024 * 1024 {
        format!("{} kB", (n / 1024).max(1))
    } else {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    }
}

// ----------------------------------------------------------------------
// Impostazioni
// ----------------------------------------------------------------------

#[derive(Default)]
pub struct SettingsState {
    cfg: Option<MailConfig>,
    tpl: Option<MailTemplates>,
    password: String,
    which: usize,
}

const TEMPLATE_NAMES: [&str; 5] = ["Invio di un documento", "Promemoria di pagamento", "Secondo sollecito", "Ultimo avviso", "Comunicazione"];

fn template_mut(t: &mut MailTemplates, i: usize) -> &mut MailTemplate {
    match i {
        0 => &mut t.document,
        1..=3 => &mut t.reminders[i - 1],
        _ => &mut t.communication,
    }
}

pub fn settings_tab(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut SettingsState) {
    let c = st.cfg.get_or_insert_with(|| mailer::mail_config(cx.store));
    ui.label("Casella da cui partono fatture, preventivi, solleciti e comunicazioni ai clienti. Le fatture elettroniche continuano a passare dalla PEC verso il Sistema di Interscambio.");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(theme::medium("Invia con"));
        for m in [MailMode::Smtp, MailMode::Pec, MailMode::MailClient] {
            ui.radio_value(&mut c.mode, m, m.label());
        }
    });
    match c.mode {
        MailMode::Pec => {
            ui.label(theme::muted("Si usa il server della casella PEC impostata nella scheda «PEC e Sistema di Interscambio». Attenzione: alcuni gestori PEC rifiutano i messaggi verso indirizzi email ordinari."));
        }
        MailMode::MailClient => {
            ui.label(theme::muted("Il messaggio, con gli allegati, si apre come nuova email nel programma di posta del computer (Outlook, Posta di Windows): lo si controlla e lo si invia da lì. Serve solo l'indirizzo del mittente."));
        }
        MailMode::Smtp => {
            ui.horizontal_wrapped(|ui| {
                ui.label(theme::muted("Servizio:"));
                for (name, host, port, tls) in mail::smtp_presets() {
                    if ui.small_button(name).clicked() {
                        c.smtp_host = host.into();
                        c.smtp_port = port;
                        c.smtp_tls = tls;
                    }
                }
            });
            ui.label(theme::muted("Gmail e Outlook.com con la verifica in due passaggi richiedono una «password per le app» creata nelle impostazioni dell'account.").size(12.0));
        }
    }
    ui.add_space(6.0);
    egui::Grid::new("mail_cfg").num_columns(4).spacing([10.0, 8.0]).show(ui, |ui| {
        ui.label("Nome del mittente");
        ui.add(egui::TextEdit::singleline(&mut c.from_name).desired_width(260.0));
        ui.label("Rispondi a (facoltativo)");
        ui.add(egui::TextEdit::singleline(&mut c.reply_to).desired_width(220.0).hint_text("amministrazione@..."));
        ui.end_row();
        if c.mode != MailMode::Pec {
            ui.label("Indirizzo email");
            let addr_color = if c.address.is_empty() || mail::is_valid_address(&c.address) { theme::INK } else { theme::NEGATIVE };
            ui.add(egui::TextEdit::singleline(&mut c.address).desired_width(260.0).text_color(addr_color));
            if c.mode == MailMode::Smtp {
                ui.label("Utente (se diverso)");
                ui.add(egui::TextEdit::singleline(&mut c.username).desired_width(220.0));
            } else {
                ui.label("");
                ui.label("");
            }
            ui.end_row();
        }
        if c.mode == MailMode::Smtp {
            ui.label("Server SMTP / porta");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut c.smtp_host).desired_width(190.0));
                let mut p = Decimal::from(c.smtp_port);
                if widgets::dec_field(ui, "mail_port", &mut p, 54.0, 0) {
                    c.smtp_port = rust_decimal::prelude::ToPrimitive::to_u16(&p).unwrap_or(465);
                }
            });
            ui.label("Sicurezza");
            ui.horizontal(|ui| {
                ui.radio_value(&mut c.smtp_tls, TlsMode::Implicit, "SSL/TLS (465)");
                ui.radio_value(&mut c.smtp_tls, TlsMode::StartTls, "STARTTLS (587)");
            });
            ui.end_row();
            ui.label("Password");
            let stored = mailer::load_password(&c.address).is_some();
            ui.add(egui::TextEdit::singleline(&mut st.password).password(true).desired_width(200.0).hint_text(if stored { "memorizzata" } else { "non memorizzata" }));
            ui.label("");
            ui.label("");
            ui.end_row();
        }
        ui.label("");
        ui.checkbox(&mut c.bcc_self, "Mandami una copia nascosta di ogni messaggio");
        ui.label("");
        ui.label("");
        ui.end_row();
    });
    ui.add_space(4.0);
    ui.label(theme::medium("Firma"));
    ui.add(egui::TextEdit::multiline(&mut c.signature).desired_width(520.0).desired_rows(3).hint_text("Se vuota: ragione sociale, telefono ed email dell'azienda"));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if theme::primary(ui, icon::FLOPPY_DISK, "Salva").clicked() && cx.msgs.check(cx.store.set_setting("mail", c), "Impostazioni email salvate").is_some() && !st.password.is_empty() && c.mode == MailMode::Smtp {
            match mailer::store_password(&c.address, &st.password) {
                Ok(_) => cx.msgs.ok("Password memorizzata nel Gestore credenziali di Windows"),
                Err(e) => cx.msgs.err(format!("Password non memorizzata: {}", e)),
            }
            st.password.clear();
        }
        if c.mode != MailMode::MailClient && theme::button(ui, icon::PLUGS_CONNECTED, "Prova connessione").clicked() {
            match c.account(&services::pec_config(cx.store)) {
                Ok(acc) => {
                    let pw = if !st.password.is_empty() {
                        Some(st.password.clone())
                    } else if c.mode == MailMode::Pec {
                        crate::config::load_password(&services::pec_config(cx.store).address)
                    } else {
                        mailer::load_password(&acc.address)
                    };
                    match pw {
                        Some(pw) => cx.nav.push(Nav::MailTest(acc, pw)),
                        None => cx.msgs.warn("Inserire la password"),
                    }
                }
                Err(e) => cx.msgs.err(e.to_string()),
            }
        }
        if c.mode == MailMode::Smtp && theme::ghost(ui, true, icon::TRASH, "Dimentica password").clicked() {
            mailer::delete_password(&c.address);
            cx.msgs.ok("Password rimossa");
        }
    });

    ui.add_space(16.0);
    theme::rule(ui);
    ui.add_space(10.0);
    ui.label(theme::bold("Testi dei messaggi").size(18.0));
    let t = st.tpl.get_or_insert_with(|| mailer::templates(cx.store));
    ui.horizontal(|ui| {
        ui.label("Solleciti dopo");
        let mut g = Decimal::from(t.grace_days);
        if widgets::dec_field(ui, "grace", &mut g, 40.0, 0) {
            t.grace_days = rust_decimal::prelude::ToPrimitive::to_u32(&g).unwrap_or(5);
        }
        ui.label("giorni dalla scadenza, a distanza di almeno");
        let mut m = Decimal::from(t.min_days_between);
        if widgets::dec_field(ui, "min_between", &mut m, 40.0, 0) {
            t.min_days_between = rust_decimal::prelude::ToPrimitive::to_u32(&m).unwrap_or(7);
        }
        ui.label("giorni l'uno dall'altro.");
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        for (i, n) in TEMPLATE_NAMES.iter().enumerate() {
            ui.selectable_value(&mut st.which, i, *n);
        }
    });
    ui.add_space(4.0);
    let which = st.which;
    let def = MailTemplates::default();
    let tm = template_mut(t, which);
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        ui.label(theme::muted("Oggetto"));
        ui.add(egui::TextEdit::singleline(&mut tm.subject).desired_width(f32::INFINITY));
        ui.label(theme::muted("Testo"));
        ui.add(egui::TextEdit::multiline(&mut tm.body).desired_width(f32::INFINITY).desired_rows(16).margin(egui::Margin::symmetric(10, 8)));
        let ui = &mut cols[1];
        ui.label(theme::medium("Segnaposto"));
        ui.label(theme::muted("Si sostituiscono con i dati di ogni messaggio.").size(12.0));
        let mut list: Vec<(&str, &str)> = mailtext::COMMON_PLACEHOLDERS.to_vec();
        match which {
            0 => list.extend_from_slice(mailtext::DOCUMENT_PLACEHOLDERS),
            1..=3 => list.extend_from_slice(mailtext::REMINDER_PLACEHOLDERS),
            _ => {}
        }
        for (k, d) in list {
            ui.horizontal(|ui| {
                ui.label(RichText::new(k).monospace().color(theme::ACCENT));
                ui.label(theme::muted(d).size(12.0));
            });
        }
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if theme::primary(ui, icon::FLOPPY_DISK, "Salva i testi").clicked() {
            cx.msgs.check(cx.store.set_setting("mail_templates", &*t), "Testi dei messaggi salvati");
        }
        if theme::ghost(ui, true, icon::ARROW_COUNTER_CLOCKWISE, "Ripristina questo testo").clicked() {
            let d = match which {
                0 => def.document.clone(),
                1..=3 => def.reminders[which - 1].clone(),
                _ => def.communication.clone(),
            };
            *template_mut(t, which) = d;
        }
    });
}
