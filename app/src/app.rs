//! Finestra principale: archivio aperto, barra dei menu, schermate e lavori PEC.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui;
use openfatt_core::model::{DocKind, PartyRole};
use openfatt_store::Store;

use crate::config::{self, AppConfig};
use crate::mailer::{self, MailJobResult};
use crate::services::{self, PecJobResult};
use crate::ui::theme::{self, icon};
use crate::ui::{self, Ctx, DocGroup, Msgs, Nav, Screen};

enum PendingPec {
    Send(i64),
    Fetch,
}

/// Clic simulati per il collaudo automatico dell'interfaccia.
#[derive(Default)]
struct ScriptedClicks {
    points: Vec<egui::Pos2>,
    step: usize,
    at: Option<std::time::Instant>,
}

impl ScriptedClicks {
    fn from_env() -> Self {
        let points = std::env::var("OPENFATT_CLICKS").unwrap_or_default().split(';').filter_map(|p| p.split_once(',')).filter_map(|(x, y)| Some(egui::pos2(x.trim().parse().ok()?, y.trim().parse().ok()?))).collect();
        ScriptedClicks { points, step: 0, at: Some(std::time::Instant::now() + std::time::Duration::from_millis(2500)) }
    }
    fn pending(&self) -> bool {
        self.step / 3 < self.points.len()
    }
    fn ready(&self) -> bool {
        self.pending() && self.at.map(|t| std::time::Instant::now() >= t).unwrap_or(false)
    }
    fn next_events(&mut self) -> Option<Vec<egui::Event>> {
        let pos = *self.points.get(self.step / 3)?;
        let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let ev = match self.step % 3 {
            0 => egui::Event::PointerMoved(pos),
            1 => button(true),
            _ => button(false),
        };
        let same = self.points.get(self.step / 3 + 1) == Some(&pos);
        self.at = Some(std::time::Instant::now() + std::time::Duration::from_millis(if self.step % 3 == 2 && !same { 900 } else if self.step % 3 == 2 { 30 } else { 40 }));
        self.step += 1;
        Some(vec![ev])
    }
}

pub struct OpenFattApp {
    /// Installatore da avviare alla chiusura del programma
    update_after_exit: Option<PathBuf>,
    cards: ui::quick::Cards,
    updates: crate::updates::Checker,
    backups: crate::backups::Manager,
    backup_ui: ui::backup::State,
    closing: bool,
    force_quit: bool,
    scripted: ScriptedClicks,
    cfg: AppConfig,
    store: Option<Store>,
    screen: Screen,
    msgs: Msgs,
    nav: Vec<Nav>,
    home: ui::home::State,
    parties: ui::parties::State,
    products: ui::products::State,
    pricelists: ui::pricelists::State,
    docs: ui::documents::State,
    counter: ui::counter::State,
    editor: Option<ui::editor::Editor>,
    einv: ui::einvoices::State,
    ledger: ui::ledger::State,
    bank: ui::bank::State,
    vat: ui::vat::State,
    stock: ui::stock::State,
    inventory: ui::inventory::State,
    settings: ui::settings::State,
    tools: ui::tools::State,
    biz: ui::business::State,
    /// Utente collegato (None se l'archivio non ha utenti)
    user: Option<openfatt_store::User>,
    login: (String, String, String),
    needs_login: bool,
    heartbeat: std::time::Instant,
    pec_tx: Sender<PecJobResult>,
    pec_rx: Receiver<PecJobResult>,
    pec_busy: Option<String>,
    session_password: Option<String>,
    password_prompt: Option<(PendingPec, String, bool)>,
    new_archive_name: String,
    badges: ui::shell::Badges,
    search: ui::shell::Search,
    composer: ui::mail::Composer,
    labels: ui::print::LabelDialog,
    mail_tx: Sender<MailJobResult>,
    mail_rx: Receiver<MailJobResult>,
}

impl OpenFattApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);

        let (pec_tx, pec_rx) = channel();
        let (mail_tx, mail_rx) = channel();
        let cfg = AppConfig::load();
        let mut app = OpenFattApp {
            update_after_exit: None,
            cards: Default::default(),
            updates: crate::updates::Checker::start(&cc.egui_ctx),
            cfg,
            store: None,
            screen: Screen::Home,
            msgs: Msgs::default(),
            nav: Vec::new(),
            home: Default::default(),
            parties: Default::default(),
            products: Default::default(),
            pricelists: Default::default(),
            docs: Default::default(),
            counter: Default::default(),
            editor: None,
            einv: Default::default(),
            ledger: Default::default(),
            bank: Default::default(),
            vat: Default::default(),
            stock: Default::default(),
            inventory: Default::default(),
            settings: Default::default(),
            tools: Default::default(),
            biz: Default::default(),
            user: None,
            login: Default::default(),
            needs_login: false,
            heartbeat: std::time::Instant::now(),
            scripted: ScriptedClicks::from_env(),
            backups: crate::backups::Manager::load(),
            backup_ui: Default::default(),
            closing: false,
            force_quit: false,
            pec_tx,
            pec_rx,
            pec_busy: None,
            session_password: None,
            password_prompt: None,
            new_archive_name: "La mia azienda".into(),
            badges: Default::default(),
            search: Default::default(),
            composer: Default::default(),
            labels: Default::default(),
            mail_tx,
            mail_rx,
        };
        // OPENFATT_ARCHIVE / OPENFATT_SCREEN: apertura diretta (utile per collaudi e collegamenti sul desktop)
        if let Some(p) = std::env::var_os("OPENFATT_ARCHIVE").map(PathBuf::from) {
            app.open_archive(p);
        } else if let Some(last) = app.cfg.last.clone() {
            if last.exists() {
                app.open_archive(last);
            }
        }
        if let Ok(q) = std::env::var("OPENFATT_SEARCH") {
            app.search.text = q;
        }
        if app.store.is_some() {
            if let Ok(s) = std::env::var("OPENFATT_SCREEN") {
                let screen = match s.as_str() {
                    "clienti" => Some(Screen::Parties(PartyRole::Customer)),
                    "fornitori" => Some(Screen::Parties(PartyRole::Supplier)),
                    "articoli" => Some(Screen::Products),
                    "listini" => Some(Screen::PriceLists),
                    x if x.starts_with("listini:") => {
                        app.pricelists = ui::pricelists::State::preset(app.store.as_ref().unwrap(), x.trim_start_matches("listini:"));
                        Some(Screen::PriceLists)
                    }
                    "fatture" => Some(Screen::Documents(DocGroup::Invoices)),
                    "ordini-fornitori" => Some(Screen::Documents(DocGroup::SupplierOrders)),
                    "preventivi-fornitori" => Some(Screen::Documents(DocGroup::SupplierQuotes)),
                    "autofatture" => Some(Screen::Documents(DocGroup::SelfInvoices)),
                    "preventivi" => Some(Screen::Documents(DocGroup::Quotes)),
                    "ddt" => Some(Screen::Documents(DocGroup::DeliveryNotes)),
                    "fe-inviate" => Some(Screen::EInvoicesOut),
                    "fe-ricevute" => Some(Screen::EInvoicesIn),
                    "scadenzario" => Some(Screen::Dues),
                    "primanota" => Some(Screen::Cash),
                    "magazzino" => Some(Screen::Stock),
                    "inventario" => {
                        app.inventory = ui::inventory::State::latest(app.store.as_ref().unwrap());
                        Some(Screen::Inventory)
                    }
                    "impostazioni" => Some(Screen::Settings),
                    "impostazioni-email" => {
                        app.settings = ui::settings::State::on_mail_tab();
                        Some(Screen::Settings)
                    }
                    x if x.starts_with("distinte") => {
                        app.bank = ui::bank::State::on_tab(ui::bank::Tab::from_name(x.trim_start_matches("distinte").trim_start_matches('-')));
                        Some(Screen::Bank)
                    }
                    x if x == "iva" || x.starts_with("iva-") => {
                        app.vat = ui::vat::State::on_tab(ui::vat::Tab::from_name(x.trim_start_matches("iva").trim_start_matches('-')));
                        Some(Screen::Vat)
                    }
                    "impostazioni-stampe" => {
                        app.settings = ui::settings::State::on_print_tab();
                        Some(Screen::Settings)
                    }
                    "impostazioni-banca" => {
                        app.settings = ui::settings::State::on_bank_tab();
                        Some(Screen::Settings)
                    }
                    "solleciti" => {
                        app.ledger.show_reminders();
                        Some(Screen::Dues)
                    }
                    "banco" => Some(Screen::Counter),
                    "editor-stampe" => {
                        app.settings = ui::settings::State::on_print_tab();
                        app.settings.print_state().layouts.start_new(false);
                        Some(Screen::Settings)
                    }
                    "ricorrenti" => Some(Screen::Extra(ui::business::Extra::Recurring)),
                    "agenti" => Some(Screen::Extra(ui::business::Extra::Agents)),
                    "statistiche" => Some(Screen::Extra(ui::business::Extra::Stats)),
                    "agenda" => Some(Screen::Extra(ui::business::Extra::Agenda)),
                    "carte" => Some(Screen::Extra(ui::business::Extra::Cards)),
                    "negozio" => Some(Screen::Extra(ui::business::Extra::Shop)),
                    "fineanno" => Some(Screen::Extra(ui::business::Extra::YearEnd)),
                    "copie" => Some(Screen::Backup),
                    "conservazione" => Some(Screen::Extra(ui::business::Extra::Conservation)),
                    "produzione" => Some(Screen::Documents(DocGroup::Production)),
                    "utenti" => {
                        app.settings = ui::settings::State::on_users_tab();
                        Some(Screen::Settings)
                    }
                    x if x.starts_with("doc:") => {
                        if let Ok(id) = x.trim_start_matches("doc:").parse::<i64>() {
                            app.nav.push(Nav::OpenDocument(id));
                            app.handle_nav();
                        }
                        None
                    }
                    x if x.starts_with("magazzino:") => {
                        let arg = x.trim_start_matches("magazzino:");
                        let (prod, trace) = match arg.split_once('|') {
                            Some((a, b)) => (Some(a), Some(b)),
                            None => (Some(arg), None),
                        };
                        app.stock = ui::stock::State::preset(app.store.as_ref().unwrap(), prod, trace);
                        Some(Screen::Stock)
                    }
                    "banco-vendite" | "banco-corrispettivi" => {
                        app.counter = ui::counter::State::on_tab(s.trim_start_matches("banco-"));
                        Some(Screen::Counter)
                    }
                    "firma" => {
                        app.settings = ui::settings::State::on_sign_tab();
                        Some(Screen::Settings)
                    }
                    "strumenti" => Some(Screen::Tools),
                    _ => None,
                };
                if let Some(sc) = screen {
                    app.screen = sc;
                } else if let Some(id) = s.strip_prefix("documento:").and_then(|x| x.parse().ok()) {
                    app.nav.push(Nav::OpenDocument(id));
                    app.handle_nav();
                }
            }
        }
        app
    }

    /// Copia di sicurezza dell'archivio, poi chiusura e aggiornamento.
    fn install_update(&mut self, ctx: &egui::Context) {
        if self.screen == Screen::Editor && self.editor.as_ref().is_some_and(|e| e.dirty) {
            self.msgs.warn("Il documento aperto ha modifiche non salvate: salvarlo prima di aggiornare");
            return;
        }
        let (installer, version) = match self.updates.checked_installer() {
            Ok(x) => x,
            Err(e) => {
                self.msgs.err(e);
                return;
            }
        };
        if let Some(store) = &self.store {
            let stem = store.path().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let dest = config::backup_dir().join(format!("{} prima di {}.gadb", stem, version));
            if let Err(e) = openfatt_store::snapshot_archive(store.path(), &dest).and_then(|_| openfatt_store::check_archive(&dest)) {
                self.msgs.err(format!("Copia di sicurezza non riuscita, aggiornamento annullato: {}", e));
                return;
            }
        }
        self.update_after_exit = Some(installer);
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Alla chiusura della finestra: si fa la copia e, se qualcosa è fermo da troppo, si avvisa.
    fn closing_flow(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.force_quit && self.store.is_some() && !self.needs_login && self.backups.cfg.enabled && !self.backups.active().is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if !self.closing {
                self.closing = true;
                // sempre, anche subito dopo un'altra copia: gli ultimi minuti di lavoro contano
                if !self.backups.running() {
                    self.backups.start();
                }
            }
        }
        if !self.closing {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        self.backups.poll(&mut self.msgs);
        let running = self.backups.running();
        let problems = if running { Vec::new() } else { self.backups.overdue() };
        let fatal = self.backups.fatal.clone();
        if !running && problems.is_empty() && fatal.is_none() {
            self.force_quit = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let mut quit = false;
        let mut retry = false;
        let mut stay = false;
        egui::Window::new("Copia di sicurezza").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.set_width(460.0);
            if running {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(self.backups.progress_text());
                });
                ui.label(theme::muted("Il programma si chiude da solo appena la copia è verificata."));
                ui.add_space(6.0);
                if ui.button("Chiudi senza attendere").clicked() {
                    quit = true;
                }
            } else {
                ui.label(egui::RichText::new("Alcune copie non sono aggiornate:").color(theme::NEGATIVE));
                if let Some(f) = &fatal {
                    ui.label(f);
                }
                for (n, why) in &problems {
                    ui.label(format!("{}: {}", n, why));
                }
                ui.label(theme::muted("Collega il disco o controlla la connessione e riprova."));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::primary(ui, theme::icon::ARROWS_CLOCKWISE, "Riprova").clicked() {
                        retry = true;
                    }
                    if theme::button(ui, "", "Torna al programma").clicked() {
                        stay = true;
                    }
                    if theme::button(ui, "", "Chiudi comunque").clicked() {
                        quit = true;
                    }
                });
            }
        });
        if quit {
            self.force_quit = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if stay {
            self.closing = false;
            self.update_after_exit = None;
        }
        if retry {
            self.backups.fatal = None;
            self.backups.start();
        }
    }

    /// Chiude l'archivio aperto: copia di sicurezza del giorno e fine della sessione.
    fn close_store(&mut self) {
        if let Some(s) = self.store.take() {
            let _ = s.end_session();
            if let Some(p) = config::auto_backup(&s, 15) {
                eprintln!("copia di sicurezza: {}", p.display());
            }
        }
        self.user = None;
        self.needs_login = false;
    }

    fn open_archive(&mut self, path: PathBuf) {
        self.close_store();
        match Store::open(&path) {
            Ok(s) => {
                let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "postazione".into());
                self.needs_login = s.has_users().unwrap_or(false) && std::env::var_os("OPENFATT_SCREEN").is_none();
                if !self.needs_login {
                    let _ = s.start_session(&host);
                }
                self.backups.on_archive_open(&s);
                self.store = Some(s);
                self.cfg.touch(&path);
                self.screen = Screen::Home;
                self.editor = None;
                self.session_password = None;
                self.reset_states();
                self.msgs.ok(format!("Archivio aperto: {}", path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()));
            }
            Err(e) => self.msgs.err(format!("Impossibile aprire {}: {}", path.display(), e)),
        }
    }

    fn reset_states(&mut self) {
        self.home = Default::default();
        self.parties = Default::default();
        self.products = Default::default();
        self.pricelists = Default::default();
        self.docs = Default::default();
        self.counter = Default::default();
        self.einv = Default::default();
        self.ledger = Default::default();
        self.stock = Default::default();
        self.inventory = Default::default();
        self.settings = Default::default();
        self.tools = Default::default();
        self.biz = Default::default();
    }

    fn password(&self, store: &Store) -> Option<String> {
        if let Some(p) = &self.session_password {
            return Some(p.clone());
        }
        let cfg = services::pec_config(store);
        config::load_password(&cfg.address)
    }

    fn start_pec(&mut self, action: PendingPec) {
        let Some(store) = &self.store else { return };
        if self.pec_busy.is_some() {
            self.msgs.warn("Un'operazione PEC è già in corso");
            return;
        }
        let cfg = services::pec_config(store);
        if let Err(e) = cfg.validate() {
            self.msgs.err(format!("{} (Opzioni > PEC)", e));
            return;
        }
        let Some(pw) = self.password(store) else {
            self.password_prompt = Some((action, String::new(), true));
            return;
        };
        match action {
            PendingPec::Send(id) => match store.einvoice(id) {
                Ok(e) => {
                    if e.is_pa() && !e.is_signed() {
                        self.msgs.err("Le fatture verso la PA vanno firmate digitalmente prima dell'invio (pulsante 'Firma')");
                        return;
                    }
                    let (name, data) = e.payload();
                    self.pec_busy = Some(format!("Invio di {} allo SdI...", name));
                    services::spawn_send(cfg, pw, id, name.to_string(), data.to_vec(), self.pec_tx.clone());
                }
                Err(e) => self.msgs.err(e.to_string()),
            },
            PendingPec::Fetch => {
                self.pec_busy = Some("Lettura della casella PEC...".into());
                services::spawn_fetch(cfg, pw, self.pec_tx.clone());
            }
        }
    }

    fn poll_pec(&mut self, ctx: &egui::Context) {
        while let Ok(r) = self.pec_rx.try_recv() {
            self.pec_busy = None;
            let Some(store) = &self.store else { continue };
            match r {
                PecJobResult::Sent { einvoice_id, result } => match result {
                    Ok(note) => {
                        let _ = store.mark_einvoice_sent(einvoice_id, &note);
                        self.msgs.ok(format!("{}. Le notifiche SdI arriveranno nella casella PEC.", note));
                    }
                    Err(e) => {
                        self.session_password = None;
                        self.msgs.err(e);
                    }
                },
                PecJobResult::Fetched(result) => match result {
                    Ok(msgs) => {
                        let sum = services::process_inbound(store, msgs);
                        if sum.errors.is_empty() {
                            self.msgs.ok(sum.text());
                        } else {
                            self.msgs.err(sum.text());
                        }
                    }
                    Err(e) => {
                        self.session_password = None;
                        self.msgs.err(e);
                    }
                },
                PecJobResult::Tested(result) => match result {
                    Ok(s) => self.msgs.ok(s),
                    Err(e) => self.msgs.err(e),
                },
            }
            ctx.request_repaint();
        }
        if self.pec_busy.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
    }

    fn handle_nav(&mut self) {
        if self.nav.is_empty() {
            return;
        }
        for _ in 0..3 {
            if self.nav.is_empty() {
                break;
            }
            self.handle_nav_once();
        }
        self.badges.invalidate();
    }

    fn handle_nav_once(&mut self) {
        let navs: Vec<Nav> = std::mem::take(&mut self.nav);
        for n in navs {
            match n {
                Nav::Go(s) => {
                    if s != Screen::Editor && self.screen == Screen::Editor && self.editor.as_ref().map(|e| e.dirty).unwrap_or(false) {
                        self.msgs.warn("Il documento aperto ha modifiche non salvate: salvarlo o chiuderlo");
                        continue;
                    }
                    if let Some(u) = &self.user {
                        let area = ui::screen_area(s);
                        if !u.can_read(area) {
                            self.msgs.err(format!("L'utente {} non ha accesso a: {}", u.username, area.label()));
                            continue;
                        }
                    }
                    self.screen = s;
                }
                Nav::OpenDocument(id) => {
                    if let Some(store) = &self.store {
                        match store.document(id) {
                            Ok(d) => {
                                if let Some(e) = &self.editor {
                                    let _ = store.release_lock("document", e.doc.id);
                                }
                                let area = ui::kind_area(d.kind);
                                if let Some(u) = &self.user {
                                    if !u.can_read(area) {
                                        self.msgs.err(format!("L'utente {} non ha accesso a: {}", u.username, area.label()));
                                        continue;
                                    }
                                }
                                let mut ed = ui::editor::Editor::new(store, d);
                                if let Some(u) = &self.user {
                                    if !u.can_write(area) {
                                        ed.set_readonly("permessi di sola lettura");
                                    }
                                }
                                match store.acquire_lock("document", id) {
                                    Ok(Some(who)) => ed.set_readonly(&format!("aperto in modifica da {}", who)),
                                    Ok(None) => {}
                                    Err(e) => self.msgs.err(e.to_string()),
                                }
                                self.editor = Some(ed);
                                self.screen = Screen::Editor;
                            }
                            Err(e) => self.msgs.err(e.to_string()),
                        }
                    }
                }
                Nav::NewDocument(kind) => {
                    if let Some(store) = &self.store {
                        self.editor = Some(ui::editor::Editor::new(store, openfatt_core::model::Document::new(kind)));
                        self.screen = Screen::Editor;
                    }
                }
                Nav::EditDocument(d) => {
                    if let Some(store) = &self.store {
                        self.editor = Some(ui::editor::Editor::new(store, d));
                        self.screen = Screen::Editor;
                    }
                }
                Nav::CloseEditor => {
                    if let (Some(store), Some(e)) = (&self.store, &self.editor) {
                        let _ = store.release_lock("document", e.doc.id);
                    }
                    let back = self.editor.as_ref().map(|e| e.back_screen()).unwrap_or(Screen::Home);
                    self.editor = None;
                    self.screen = back;
                    self.docs.dirty = true;
                }
                Nav::PecSend(id) => self.start_pec(PendingPec::Send(id)),
                Nav::PecFetch => self.start_pec(PendingPec::Fetch),
                Nav::PecTest(cfg, pw) => {
                    if self.pec_busy.is_none() {
                        self.pec_busy = Some("Verifica della connessione PEC...".into());
                        services::spawn_test(cfg, pw, self.pec_tx.clone());
                    }
                }
                Nav::OpenArchive(p) => self.open_archive(p),
                Nav::NewArchive(p) => {
                    if p.exists() {
                        self.msgs.err(format!("Il file {} esiste già", p.display()));
                    } else {
                        self.open_archive(p);
                    }
                }
                Nav::CloseArchive => {
                    self.close_store();
                    self.editor = None;
                    self.cfg.last = None;
                    self.cfg.save();
                }
                Nav::Refresh => self.reset_states(),
                Nav::FindParty(role, id, name) => {
                    self.parties.focus(id, &name);
                    self.nav.push(Nav::Go(Screen::Parties(role)));
                }
                Nav::FindProduct(id, code) => {
                    self.products.focus(id, &code);
                    self.nav.push(Nav::Go(Screen::Products));
                }
                Nav::CounterTab(tab) => {
                    self.counter = ui::counter::State::on_tab(tab);
                    self.nav.push(Nav::Go(Screen::Counter));
                }
                Nav::Mail(list) => {
                    if list.is_empty() {
                        self.msgs.warn("Nessun messaggio da preparare");
                    } else {
                        self.composer.open(list);
                    }
                }
                Nav::MailTest(acc, pw) => {
                    self.msgs.ok(format!("Verifica di {} in corso...", acc.host));
                    mailer::spawn_test(acc, pw, self.mail_tx.clone());
                }
                Nav::BankSettings => {
                    self.settings = ui::settings::State::on_bank_tab();
                    self.nav.push(Nav::Go(Screen::Settings));
                }
                Nav::Labels(list) => {
                    if list.is_empty() {
                        self.msgs.warn("Nessuna etichetta da stampare");
                    } else if let Some(store) = &self.store {
                        self.labels.open(store, list);
                    }
                }
                Nav::PrintSettings => {
                    self.settings = ui::settings::State::on_print_tab();
                    self.nav.push(Nav::Go(Screen::Settings));
                }
                Nav::VatTab(t) => {
                    self.vat = ui::vat::State::on_tab(t);
                    self.nav.push(Nav::Go(Screen::Vat));
                }
                Nav::BankTab(t) => {
                    self.bank = ui::bank::State::on_tab(t);
                    self.nav.push(Nav::Go(Screen::Bank));
                }
                Nav::MailSettings => {
                    self.settings = ui::settings::State::on_mail_tab();
                    self.nav.push(Nav::Go(Screen::Settings));
                }
                Nav::Reminders => {
                    self.ledger.show_reminders();
                    self.nav.push(Nav::Go(Screen::Dues));
                }
                Nav::Payments(v) => {
                    self.ledger.show_view(v);
                    self.nav.push(Nav::Go(Screen::Dues));
                }
            }
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let w = ui.available_width().min(920.0);
        let pad = ((ui.available_width() - w) / 2.0).max(0.0);
        ui.add_space(48.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(w);
                ui.horizontal(|ui| {
                    theme::mark(ui, 40.0);
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(theme::bold("openFatt").size(28.0));
                        ui.label(theme::muted("Fatturazione, fatture elettroniche, magazzino e vendita al banco. Software libero.").size(15.0));
                    });
                });
                ui.add_space(28.0);
                ui.columns(2, |cols| {
                    let ui = &mut cols[0];
                    theme::card(ui, "Nuovo archivio", |_| {}, |ui| {
                        ui.set_min_height(240.0);
                        ui.label(theme::muted("Ogni azienda ha il proprio file di archivio."));
                        ui.add_space(8.0);
                        ui.label(theme::medium("Nome dell'azienda").size(13.0));
                        ui.add(egui::TextEdit::singleline(&mut self.new_archive_name).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 8)));
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
                            if theme::primary(ui, icon::PLUS, "Crea in Documenti").clicked() {
                                let safe: String = self.new_archive_name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
                                let p = config::documents_dir().join(format!("{}.gadb", safe.trim()));
                                self.nav.push(Nav::NewArchive(p));
                            }
                            if theme::button(ui, icon::FOLDER_SIMPLE_PLUS, "Scegli cartella...").clicked() {
                                if let Some(p) = rfd::FileDialog::new().add_filter("Archivio openFatt", &["gadb"]).set_file_name(format!("{}.gadb", self.new_archive_name)).save_file() {
                                    self.nav.push(Nav::NewArchive(p));
                                }
                            }
                        });
                        ui.add_space(14.0);
                        theme::rule(ui);
                        ui.add_space(10.0);
                        if theme::ghost(ui, true, icon::FLASK, "Prova con dati dimostrativi").on_hover_text("Crea un archivio di prova con clienti, articoli e documenti di esempio").clicked() {
                            let p = config::documents_dir().join(format!("Demo {}.gadb", chrono::Local::now().format("%Y%m%d-%H%M%S")));
                            match Store::open(&p).and_then(|s| crate::demo::populate(&s).map(|_| s)) {
                                Ok(_) => self.nav.push(Nav::OpenArchive(p)),
                                Err(e) => self.msgs.err(e.to_string()),
                            }
                        }
                    });
                    let ui = &mut cols[1];
                    theme::card(ui, "Apri archivio", |_| {}, |ui| {
                        ui.set_min_height(240.0);
                        if theme::button(ui, icon::FOLDER_OPEN, "Apri un file...").clicked() {
                            if let Some(p) = rfd::FileDialog::new().add_filter("Archivio openFatt", &["gadb"]).pick_file() {
                                self.nav.push(Nav::OpenArchive(p));
                            }
                        }
                        ui.add_space(12.0);
                        ui.label(theme::muted("Recenti").size(13.0));
                        if self.cfg.recent.is_empty() {
                            ui.label(theme::muted("Nessun archivio aperto di recente."));
                        }
                        for r in self.cfg.recent.clone() {
                            let exists = r.exists();
                            let name = r.file_stem().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                            let resp = ui.add_enabled(exists, egui::Button::new(theme::medium(format!("{}  {}", icon::DATABASE, name))).frame(false));
                            ui.label(theme::muted(r.parent().map(|p| p.display().to_string()).unwrap_or_default()).size(12.0));
                            if resp.clicked() {
                                self.nav.push(Nav::OpenArchive(r.clone()));
                            }
                            ui.add_space(4.0);
                        }
                    });
                });
            });
        });
    }
}

impl eframe::App for OpenFattApp {
    /// Collaudo dell'interfaccia: clic simulati letti da OPENFATT_CLICKS ("x,y;x,y", in punti).
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        if !self.scripted.ready() {
            return;
        }
        if let Some(ev) = self.scripted.next_events() {
            raw.events.extend(ev);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let (Some(store), Some(e)) = (&self.store, &self.editor) {
            let _ = store.release_lock("document", e.doc.id);
        }
        self.close_store();
        if let Some(installer) = self.update_after_exit.take() {
            if let Err(e) = crate::updates::launch_installer(&installer) {
                eprintln!("{}", e);
            }
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.scripted.pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.closing_flow(ctx);
        self.updates.poll();
        if self.store.is_some() && !self.needs_login {
            self.backups.tick(&mut self.msgs);
            ctx.request_repaint_after(std::time::Duration::from_secs(if self.backups.running() { 1 } else { 30 }));
        }
        self.poll_pec(ctx);
        while let Ok(r) = self.mail_rx.try_recv() {
            if let Some(store) = &self.store {
                self.composer.on_result(store, &mut self.msgs, r);
                self.badges.invalidate();
            }
            ctx.request_repaint();
        }
        if self.composer.in_flight > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }

        let bar = |fill| egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(20, 0));
        let pec_ok = self.store.as_ref().map(|s| services::pec_config(s).validate().is_ok()).unwrap_or(false);
        egui::TopBottomPanel::bottom("status").exact_height(30.0).frame(bar(theme::SURFACE)).show(ctx, |ui| {
            let busy = self.pec_busy.clone().or_else(|| self.composer.busy_text());
            let summary = self.store.as_ref().map(|_| self.backups.summary());
            ui::shell::status_bar(ui, self.store.as_ref(), busy.as_deref(), pec_ok, summary, &mut self.nav);
        });

        // segnale di presenza della postazione
        if self.heartbeat.elapsed().as_secs() >= 60 {
            self.heartbeat = std::time::Instant::now();
            if let Some(s) = &self.store {
                let _ = s.heartbeat();
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(60));
        if self.needs_login {
            if let Some(store) = &self.store {
                let (n, p, e) = &mut self.login;
                if let Some(u) = ui::users::login_window(ctx, store, n, p, e) {
                    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "postazione".into());
                    let _ = store.start_session(&host);
                    self.msgs.ok(format!("Benvenuto, {}", if u.full_name.is_empty() { u.username.clone() } else { u.full_name.clone() }));
                    self.user = Some(u);
                    self.needs_login = false;
                }
                return;
            }
        }
        if self.store.is_none() {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::GROUND).inner_margin(24)).show(ctx, |ui| self.welcome(ui));
        } else {
            let store = self.store.as_ref().unwrap();
            self.badges.refresh(store);
            let editor = self.editor.as_ref().map(|e| (format!("{} {}", e.doc.kind.label(), e.doc.number_label()), e.dirty));
            egui::TopBottomPanel::top("toolbar").exact_height(62.0).frame(egui::Frame::new().fill(theme::SURFACE).inner_margin(egui::Margin::symmetric(6, 0))).show(ctx, |ui| {
                ui::shell::tool_bar(ui, store, &self.cfg.recent, &mut self.search, self.pec_busy.is_some(), self.screen, editor, &self.badges, &mut self.nav);
            });
            let can_update = self.user.as_ref().is_none_or(|u| u.can_write(openfatt_store::Area::Settings));
            match self.updates.banner(ctx, can_update) {
                Some(crate::updates::Action::Download) => self.updates.download(ctx),
                Some(crate::updates::Action::Install) => self.install_update(ctx),
                Some(crate::updates::Action::OpenPage) => self.updates.open_page(),
                Some(crate::updates::Action::Hide) => self.updates.hide(),
                None => {}
            }
            if let Some(msg) = self.backups.banner() {
                let mut hide = false;
                egui::TopBottomPanel::top("backup_banner").frame(egui::Frame::new().fill(egui::Color32::from_rgb(0xff, 0xf1, 0xd6)).inner_margin(egui::Margin::symmetric(12, 5))).show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("{}  {}", theme::icon::WARNING, msg)).color(theme::WARNING));
                        if theme::ghost(ui, true, theme::icon::ARROW_RIGHT, "Copie di sicurezza").clicked() {
                            self.nav.push(Nav::Go(Screen::Backup));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Più tardi").clicked() {
                                hide = true;
                            }
                        });
                    });
                });
                if hide {
                    self.backups.banner_hidden = true;
                }
            }
            egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::GROUND).inner_margin(egui::Margin { left: 8, right: 8, top: 8, bottom: 6 })).show(ctx, |ui| {
                let store = self.store.as_ref().unwrap();
                let mut cx = Ctx { store, msgs: &mut self.msgs, nav: &mut self.nav };
                match self.screen {
                    Screen::Home => ui::home::show(ui, &mut cx, &mut self.home),
                    Screen::Parties(role) => ui::parties::show(ui, &mut cx, &mut self.parties, role),
                    Screen::Products => ui::products::show(ui, &mut cx, &mut self.products),
                    Screen::PriceLists => ui::pricelists::show(ui, &mut cx, &mut self.pricelists),
                    Screen::Documents(g) => ui::documents::show(ui, &mut cx, &mut self.docs, g),
                    Screen::Counter => ui::counter::show(ui, &mut cx, &mut self.counter),
                    Screen::Editor => match &mut self.editor {
                        Some(e) => e.show(ui, &mut cx),
                        None => {
                            cx.nav.push(Nav::Go(Screen::Home));
                        }
                    },
                    Screen::EInvoicesOut => ui::einvoices::show(ui, &mut cx, &mut self.einv, openfatt_store::Direction::Out),
                    Screen::EInvoicesIn => ui::einvoices::show(ui, &mut cx, &mut self.einv, openfatt_store::Direction::In),
                    Screen::Dues => ui::ledger::show_dues(ui, &mut cx, &mut self.ledger),
                    Screen::Cash => ui::ledger::show_cash(ui, &mut cx, &mut self.ledger),
                    Screen::Bank => ui::bank::show(ui, &mut cx, &mut self.bank),
                    Screen::Vat => ui::vat::show(ui, &mut cx, &mut self.vat),
                    Screen::Stock => ui::stock::show(ui, &mut cx, &mut self.stock),
                    Screen::Inventory => ui::inventory::show(ui, &mut cx, &mut self.inventory),
                    Screen::Settings => ui::settings::show(ui, &mut cx, &mut self.settings),
                    Screen::Tools => ui::tools::show(ui, &mut cx, &mut self.tools),
                    Screen::Backup => ui::backup::show(ui, &mut cx, &mut self.backup_ui, &mut self.backups),
                    Screen::Extra(x) => ui::business::show(ui, &mut cx, &mut self.biz, x),
                }
                let can_view = self.user.as_ref().is_none_or(|u| u.can_read(openfatt_store::Area::Registry));
                let can_edit_stock = self.user.as_ref().is_none_or(|u| u.can_write(openfatt_store::Area::Stock));
                self.cards.show(ui.ctx(), &mut cx, can_view, can_edit_stock);
            });
        }

        // richiesta password PEC
        let mut submit = None;
        let mut cancel = false;
        if let Some((_, pw, remember)) = &mut self.password_prompt {
            egui::Window::new("Password PEC").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.label("Inserire la password della casella PEC:");
                let r = ui.add(egui::TextEdit::singleline(pw).password(true).desired_width(260.0));
                ui.checkbox(remember, "Ricorda nel Gestore credenziali di Windows");
                ui.horizontal(|ui| {
                    if ui.button("Continua").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                        submit = Some((pw.clone(), *remember));
                    }
                    if ui.button("Annulla").clicked() {
                        cancel = true;
                    }
                });
            });
        }
        if cancel {
            self.password_prompt = None;
        }
        if let Some((pw, remember)) = submit {
            if let Some((action, _, _)) = self.password_prompt.take() {
                if !pw.is_empty() {
                    if remember {
                        if let Some(store) = &self.store {
                            let addr = services::pec_config(store).address;
                            if let Err(e) = config::store_password(&addr, &pw) {
                                self.msgs.warn(format!("Password non memorizzata: {}", e));
                            }
                        }
                    }
                    self.session_password = Some(pw);
                    self.start_pec(action);
                }
            }
        }

        if let Some(store) = &self.store {
            self.composer.show(ctx, store, &mut self.msgs, &self.mail_tx);
            self.labels.show(ctx, &mut self.msgs);
        }
        self.handle_nav();
        self.msgs.show(ctx);

        // scorciatoie
        if self.store.is_some() {
            let (k, f5, f9) = ctx.input(|i| (i.modifiers.ctrl && i.key_pressed(egui::Key::K), i.key_pressed(egui::Key::F5), i.key_pressed(egui::Key::F9)));
            if k {
                self.search.focus_request = true;
            }
            if f5 {
                self.nav.push(Nav::PecFetch);
            }
            if f9 {
                self.nav.push(Nav::Go(Screen::Counter));
            }
            self.handle_nav();
        }
        if self.store.is_some() && ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::N)) && self.screen != Screen::Editor {
            let kind = match self.screen {
                Screen::Documents(g) => g.kinds()[0],
                _ => DocKind::Invoice,
            };
            self.nav.push(Nav::NewDocument(kind));
        }
    }
}
