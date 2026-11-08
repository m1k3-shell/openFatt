//! Schermata delle copie di sicurezza: destinazioni, stato, copie presenti e ripristino.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use eframe::egui::{self, RichText};
use openfatt_backup::{Dest, DestKind, Manifest};

use super::theme::{self, icon, Tone};
use super::{Ctx, Nav};
use crate::backups::{self, Manager};

#[derive(Default)]
pub struct State {
    add: Option<AddForm>,
    copies: Option<Copies>,
    confirm_remove: Option<(String, String)>,
    restored: Option<PathBuf>,
    from_file: Option<FromFile>,
    testing: Option<(String, Receiver<Result<(), String>>)>,
}

struct AddForm {
    dest: Dest,
    secret: String,
    pass1: String,
    pass2: String,
    error: String,
    rx: Option<Receiver<Result<(), String>>>,
}

struct Copies {
    dest: Dest,
    list: Vec<Manifest>,
    error: String,
    password: String,
    rx: Option<Receiver<Result<Vec<Manifest>, String>>>,
    restoring: Option<Receiver<Result<PathBuf, String>>>,
}

struct FromFile {
    file: PathBuf,
    password: String,
    error: String,
    rx: Option<Receiver<Result<PathBuf, String>>>,
}

fn new_dest(kind: DestKind, name: &str, path: String) -> AddForm {
    AddForm { dest: Dest { id: openfatt_backup::new_id(), name: name.to_string(), kind, path, enabled: true, ..Default::default() }, secret: String::new(), pass1: String::new(), pass2: String::new(), error: String::new(), rx: None }
}

fn output_path(stem: &str) -> PathBuf {
    crate::config::documents_dir().join(format!("{} ripristinato {}.gadb", stem, chrono::Local::now().format("%Y-%m-%d %H-%M-%S")))
}

fn size_text(b: u64) -> String {
    if b >= 1 << 20 {
        format!("{:.1} MB", b as f64 / (1u64 << 20) as f64)
    } else {
        format!("{} KB", b / 1024)
    }
}

fn same_disk(a: &str, b: &std::path::Path) -> bool {
    let bs = b.to_string_lossy();
    a.len() >= 2 && bs.len() >= 2 && a[..2].eq_ignore_ascii_case(&bs[..2])
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, m: &mut Manager) {
    let sub = m.summary().1;
    let running = m.running();
    theme::title_row(ui, &[], "Copie di sicurezza", Some(theme::subline(&[(sub.as_str(), false)])), |ui| {
        if theme::primary_enabled(ui, !running && !m.active().is_empty(), icon::ARROWS_CLOCKWISE, "Esegui ora").clicked() {
            m.start();
        }
    });
    if running {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(m.progress_text());
        });
    }

    // ---- pianificazione
    theme::card(ui, "Automatismi", |_| {}, |ui| {
        let before = m.cfg.clone();
        ui.horizontal(|ui| {
            ui.checkbox(&mut m.cfg.enabled, "Copie automatiche");
            ui.separator();
            ui.label("mentre si lavora, ogni");
            ui.add(egui::DragValue::new(&mut m.cfg.interval_hours).range(1..=24).suffix(" ore"));
            ui.label("e a ogni chiusura del programma.");
            ui.separator();
            ui.label("Avvisa se una destinazione è ferma da");
            ui.add(egui::DragValue::new(&mut m.cfg.alert_days).range(1..=30).suffix(" giorni"));
        });
        ui.horizontal(|ui| {
            ui.label("Conserva:");
            let r = &mut m.cfg.retention;
            ui.add(egui::DragValue::new(&mut r.recent).range(3..=100).suffix(" ultime"));
            ui.add(egui::DragValue::new(&mut r.daily).range(1..=365).suffix(" giornaliere"));
            ui.add(egui::DragValue::new(&mut r.weekly).range(1..=104).suffix(" settimanali"));
            ui.add(egui::DragValue::new(&mut r.monthly).range(1..=120).suffix(" mensili"));
            ui.add(egui::DragValue::new(&mut r.yearly).range(1..=50).suffix(" annuali"));
            ui.label(theme::muted("Si tiene sempre anche la copia più vecchia."));
        });
        if before != m.cfg {
            m.save_cfg();
        }
    });
    ui.add_space(6.0);

    // ---- destinazioni
    let mut open_copies: Option<Dest> = None;
    let mut test_dest: Option<Dest> = None;
    theme::card(
        ui,
        "Dove vengono salvate",
        |ui| {
            if theme::button(ui, icon::GLOBE, "Cloud WebDAV...").on_hover_text("Nextcloud e altri servizi con accesso WebDAV").clicked() {
                st.add = Some(new_dest(DestKind::WebDav, "Cloud", String::new()));
            }
            egui::menu::menu_custom_button(ui, theme::button_widget(icon::CLOUD, "OneDrive, Google Drive, Dropbox..."), |ui| {
                let found = backups::cloud_folders();
                for (name, path) in &found {
                    if ui.button(format!("{}  ({})", name, path.display())).clicked() {
                        ui.close_menu();
                        st.add = Some(new_dest(DestKind::CloudFolder, name, path.join("Copie openFatt").to_string_lossy().to_string()));
                    }
                }
                if found.is_empty() {
                    ui.label(theme::muted("Nessun servizio cloud trovato su questo computer"));
                }
                ui.separator();
                if ui.button("Scegli la cartella sincronizzata...").clicked() {
                    ui.close_menu();
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        st.add = Some(new_dest(DestKind::CloudFolder, "Cloud", p.join("Copie openFatt").to_string_lossy().to_string()));
                    }
                }
            });
            if theme::button(ui, icon::HARD_DRIVE, "Hard disk esterno o cartella...").clicked() {
                if let Some(p) = rfd::FileDialog::new().set_title("Scegli il disco o la cartella per le copie").pick_folder() {
                    st.add = Some(new_dest(DestKind::Folder, "Hard disk esterno", p.join("Copie openFatt").to_string_lossy().to_string()));
                }
            }
        },
        |ui| {
            let now = chrono::Local::now().naive_local();
            egui::Grid::new("backup_dests").num_columns(6).spacing([18.0, 8.0]).striped(false).show(ui, |ui| {
                for h in ["Nome", "Tipo", "Posizione", "Ultima copia verificata", "Stato", ""] {
                    ui.label(theme::medium(h).size(12.5).color(theme::MUTED));
                }
                ui.end_row();
                let list = m.cfg.dests.clone();
                for d in &list {
                    let s = m.state(&d.id);
                    ui.label(theme::medium(&d.name));
                    ui.label(format!("{}{}", d.kind.label(), if d.encrypt { " · cifrata" } else { "" }));
                    ui.label(super::widgets::truncate(if d.kind == DestKind::WebDav { &d.url } else { &d.path }, 52));
                    match s.last_ok_at() {
                        Some(t) => {
                            let days = (now - t).num_days();
                            let late = days >= m.cfg.alert_days as i64;
                            ui.label(RichText::new(format!("{} · {}", t.format("%d/%m/%Y %H:%M"), if s.copies == 1 { "1 copia".to_string() } else { format!("{} copie", s.copies) })).color(if late { theme::NEGATIVE } else { theme::INK }));
                        }
                        None => {
                            ui.label(theme::muted("mai"));
                        }
                    }
                    ui.horizontal(|ui| {
                        if !s.last_error.is_empty() {
                            theme::pill(ui, "Non riuscita", Tone::Negative).on_hover_text(&s.last_error);
                            ui.label(RichText::new(super::widgets::truncate(&s.last_error, 48)).size(12.0).color(theme::NEGATIVE));
                        } else if !s.warning.is_empty() {
                            theme::pill(ui, "Attenzione", Tone::Warning).on_hover_text(&s.warning);
                        } else if s.last_ok.is_some() {
                            theme::pill(ui, "Verificata", Tone::Positive);
                        } else {
                            theme::pill(ui, "In attesa", Tone::Neutral);
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.small_button("Copie e ripristino...").clicked() {
                            open_copies = Some(d.clone());
                        }
                        if ui.add_enabled(st.testing.is_none(), egui::Button::new("Prova").small()).on_hover_text("Scrive, rilegge e cancella un file di prova").clicked() {
                            test_dest = Some(d.clone());
                        }
                        let mut on = d.enabled;
                        if ui.checkbox(&mut on, "attiva").changed() {
                            if let Some(x) = m.cfg.dests.iter_mut().find(|x| x.id == d.id) {
                                x.enabled = on;
                            }
                            m.save_cfg();
                        }
                        if ui.small_button(icon::TRASH).on_hover_text("Rimuovi la destinazione (le copie già fatte restano dove sono)").clicked() {
                            st.confirm_remove = Some((d.id.clone(), d.name.clone()));
                        }
                    });
                    ui.end_row();
                }
                ui.label(theme::medium("Questo computer"));
                ui.label("Copia giornaliera");
                ui.label(super::widgets::truncate(&crate::config::backup_dir().display().to_string(), 52));
                ui.label(theme::muted("alla chiusura, ultime 15"));
                ui.label(theme::muted("non protegge da guasti del computer"));
                ui.label("");
                ui.end_row();
            });
            if m.cfg.dests.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new("Per non perdere i dati aggiungi almeno un hard disk esterno e un cloud: se uno si guasta resta l'altro.").color(theme::WARNING));
            }
        },
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if theme::button(ui, icon::UPLOAD_SIMPLE, "Ripristina da un file...").on_hover_text("Su un computer nuovo: scegli una copia .gadb o .gadb.enc").clicked() {
            if let Some(p) = rfd::FileDialog::new().add_filter("Copie openFatt", &["gadb", "enc"]).pick_file() {
                st.from_file = Some(FromFile { file: p, password: String::new(), error: String::new(), rx: None });
            }
        }
        ui.label(theme::muted("Il ripristino crea sempre un archivio nuovo: quello attuale non viene toccato."));
    });

    if let Some(d) = open_copies {
        let (tx, rx) = channel();
        let (stem, uid) = m.identity();
        let secret = backups::get_secret(&d.id, "accesso");
        let dd = d.clone();
        std::thread::spawn(move || {
            let _ = tx.send(openfatt_backup::list_backups(&dd, secret.as_deref(), &stem, &uid));
        });
        let password = backups::get_secret(&d.id, "cifra").unwrap_or_default();
        st.copies = Some(Copies { dest: d, list: Vec::new(), error: String::new(), password, rx: Some(rx), restoring: None });
    }
    if let Some(d) = test_dest {
        let (tx, rx) = channel();
        let secret = backups::get_secret(&d.id, "accesso");
        let name = d.name.clone();
        std::thread::spawn(move || {
            let r = if d.kind == DestKind::WebDav || openfatt_backup::reachable(&d) { openfatt_backup::prepare_and_test(&d, secret.as_deref(), &backups::spool_dir()) } else { Err("disco o cartella non disponibile".to_string()) };
            let _ = tx.send(r);
        });
        st.testing = Some((name, rx));
    }
    if let Some((name, rx)) = &st.testing {
        if let Ok(r) = rx.try_recv() {
            match r {
                Ok(_) => cx.msgs.ok(format!("{}: prova riuscita", name)),
                Err(e) => cx.msgs.err(format!("{}: {}", name, e)),
            }
            st.testing = None;
        }
    }

    add_window(ui.ctx(), cx, st, m);
    copies_window(ui.ctx(), cx, st, m);
    from_file_window(ui.ctx(), cx, st);

    if let Some((id, name)) = st.confirm_remove.clone() {
        match super::widgets::confirm(ui.ctx(), "Rimuovi destinazione", &format!("Non fare più copie su \"{}\"? Le copie già presenti restano dove sono.", name)) {
            Some(true) => {
                m.remove(&id);
                st.confirm_remove = None;
            }
            Some(false) => st.confirm_remove = None,
            None => {}
        }
    }
    if let Some(p) = st.restored.clone() {
        let mut close = false;
        egui::Window::new("Ripristino completato").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
            ui.label("La copia è stata controllata e salvata in un archivio nuovo:");
            ui.label(theme::medium(p.display().to_string()));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::primary(ui, icon::FOLDER_OPEN, "Apri l'archivio ripristinato").clicked() {
                    cx.nav.push(Nav::OpenArchive(p.clone()));
                    close = true;
                }
                if theme::button(ui, "", "Non ora").clicked() {
                    close = true;
                }
            });
        });
        if close {
            st.restored = None;
        }
    }
    if st.testing.is_some() || st.add.as_ref().is_some_and(|a| a.rx.is_some()) || st.copies.as_ref().is_some_and(|c| c.rx.is_some() || c.restoring.is_some()) || st.from_file.as_ref().is_some_and(|f| f.rx.is_some()) || running {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
}

fn add_window(ctx: &egui::Context, cx: &mut Ctx, st: &mut State, m: &mut Manager) {
    let Some(a) = &mut st.add else { return };
    let mut close = false;
    let source = cx.store.path().to_path_buf();
    egui::Window::new(match a.dest.kind {
        DestKind::Folder => "Nuova destinazione: hard disk o cartella",
        DestKind::CloudFolder => "Nuova destinazione: cloud",
        DestKind::WebDav => "Nuova destinazione: cloud WebDAV",
    })
    .collapsible(false)
    .resizable(false)
    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
    .show(ctx, |ui| {
        ui.set_width(560.0);
        egui::Grid::new("backup_add").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Nome");
            ui.add(egui::TextEdit::singleline(&mut a.dest.name).desired_width(320.0));
            ui.end_row();
            if a.dest.kind == DestKind::WebDav {
                ui.label("Indirizzo WebDAV");
                ui.add(egui::TextEdit::singleline(&mut a.dest.url).desired_width(420.0).hint_text("https://cloud.esempio.it/remote.php/dav/files/nome/Copie"));
                ui.end_row();
                ui.label("Utente");
                ui.add(egui::TextEdit::singleline(&mut a.dest.user).desired_width(240.0));
                ui.end_row();
                ui.label("Password");
                ui.add(egui::TextEdit::singleline(&mut a.secret).password(true).desired_width(240.0));
                ui.end_row();
            } else {
                ui.label("Cartella");
                ui.label(&a.dest.path);
                ui.end_row();
            }
            ui.label("");
            ui.checkbox(&mut a.dest.encrypt, "Cifra le copie con una password");
            ui.end_row();
            if a.dest.encrypt {
                ui.label("Password di cifratura");
                ui.add(egui::TextEdit::singleline(&mut a.pass1).password(true).desired_width(240.0));
                ui.end_row();
                ui.label("Ripeti la password");
                ui.add(egui::TextEdit::singleline(&mut a.pass2).password(true).desired_width(240.0));
                ui.end_row();
            }
        });
        if a.dest.encrypt {
            ui.label(RichText::new("Senza questa password le copie non si possono recuperare, nemmeno da noi. Scrivila su carta e conservala in un posto sicuro.").color(theme::NEGATIVE));
        }
        if a.dest.kind != DestKind::WebDav && same_disk(&a.dest.path, &source) {
            ui.label(RichText::new("Questa cartella è sullo stesso disco dell'archivio: se il disco si guasta si perdono entrambi.").color(theme::WARNING));
        }
        if a.dest.kind == DestKind::CloudFolder {
            ui.label(theme::muted("Il file viene verificato sul computer; il caricamento online lo fa il programma del servizio cloud, che deve restare attivo."));
        }
        if !a.error.is_empty() {
            ui.label(RichText::new(&a.error).color(theme::NEGATIVE));
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let busy = a.rx.is_some();
            if theme::primary_enabled(ui, !busy, icon::CHECK, if busy { "Prova in corso..." } else { "Prova e salva" }).clicked() {
                a.error.clear();
                if a.dest.encrypt && a.pass1.chars().count() < 8 {
                    a.error = "La password di cifratura deve avere almeno 8 caratteri".into();
                } else if a.dest.encrypt && a.pass1 != a.pass2 {
                    a.error = "Le due password non coincidono".into();
                } else if a.dest.kind == DestKind::WebDav && a.dest.url.trim().is_empty() {
                    a.error = "Indicare l'indirizzo WebDAV".into();
                } else {
                    let (tx, rx) = channel();
                    let d = a.dest.clone();
                    let secret = a.secret.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send(openfatt_backup::prepare_and_test(&d, Some(&secret), &backups::spool_dir()));
                    });
                    a.rx = Some(rx);
                }
            }
            if theme::button(ui, "", "Annulla").clicked() {
                close = true;
            }
        });
    });
    if let Some(r) = a.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
        a.rx = None;
        let saved = r.and_then(|_| {
            if a.dest.kind == DestKind::WebDav {
                backups::set_secret(&a.dest.id, "accesso", &a.secret)?;
            }
            if a.dest.encrypt {
                backups::set_secret(&a.dest.id, "cifra", &a.pass1)?;
            }
            Ok(())
        });
        match saved {
            Ok(_) => {
                cx.msgs.ok(format!("{}: destinazione provata e salvata. Prima copia in corso", a.dest.name));
                m.cfg.dests.push(a.dest.clone());
                m.save_cfg();
                m.start();
                close = true;
            }
            Err(e) => a.error = e,
        }
    }
    if close {
        st.add = None;
    }
}

fn copies_window(ctx: &egui::Context, cx: &mut Ctx, st: &mut State, m: &mut Manager) {
    let Some(c) = &mut st.copies else { return };
    if let Some(r) = c.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
        c.rx = None;
        match r {
            Ok(l) => c.list = l,
            Err(e) => c.error = e,
        }
    }
    let mut restored = None;
    if let Some(r) = c.restoring.as_ref().and_then(|rx| rx.try_recv().ok()) {
        c.restoring = None;
        match r {
            Ok(p) => restored = Some(p),
            Err(e) => c.error = e,
        }
    }
    let mut open = true;
    let mut chosen: Option<Manifest> = None;
    egui::Window::new(format!("Copie su {}", c.dest.name)).open(&mut open).collapsible(false).default_size([760.0, 460.0]).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        if c.rx.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Lettura delle copie...");
            });
        }
        if !c.error.is_empty() {
            ui.label(RichText::new(&c.error).color(theme::NEGATIVE));
        }
        if c.list.iter().any(|x| x.encrypted) {
            ui.horizontal(|ui| {
                ui.label("Password di cifratura");
                ui.add(egui::TextEdit::singleline(&mut c.password).password(true).desired_width(220.0));
            });
        }
        if c.restoring.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Ripristino e controllo della copia...");
            });
        }
        egui::ScrollArea::vertical().max_height(380.0).auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("backup_copies").num_columns(5).striped(true).spacing([18.0, 5.0]).show(ui, |ui| {
                for h in ["Data", "Dimensione", "Documenti", "Clienti e fornitori", ""] {
                    ui.label(theme::medium(h).size(12.5).color(theme::MUTED));
                }
                ui.end_row();
                for x in &c.list {
                    ui.label(format!("{}{}", x.created, if x.encrypted { "  (cifrata)" } else { "" }));
                    ui.label(size_text(x.size));
                    ui.label(x.counts.get("documents").map(|n| n.to_string()).unwrap_or_default());
                    ui.label(x.counts.get("parties").map(|n| n.to_string()).unwrap_or_default());
                    if ui.add_enabled(c.restoring.is_none(), egui::Button::new("Ripristina").small()).clicked() {
                        chosen = Some(x.clone());
                    }
                    ui.end_row();
                }
            });
        });
        if c.list.is_empty() && c.rx.is_none() && c.error.is_empty() {
            ui.label(theme::muted("Nessuna copia di questo archivio in questa destinazione."));
        }
    });
    if let Some(x) = chosen {
        let (tx, rx) = channel();
        let (stem, uid) = m.identity();
        let d = c.dest.clone();
        let secret = backups::get_secret(&d.id, "accesso");
        let pw = c.password.clone();
        let out = output_path(&stem);
        std::thread::spawn(move || {
            let r = openfatt_backup::restore(&d, secret.as_deref(), Some(&pw), &stem, &uid, &x, &out, &backups::spool_dir()).map(|_| out);
            let _ = tx.send(r);
        });
        c.error.clear();
        c.restoring = Some(rx);
    }
    let _ = cx;
    if let Some(p) = restored {
        st.restored = Some(p);
        st.copies = None;
    } else if !open {
        st.copies = None;
    }
}

fn from_file_window(ctx: &egui::Context, _cx: &mut Ctx, st: &mut State) {
    let Some(f) = &mut st.from_file else { return };
    let encrypted = f.file.extension().map(|e| e == "enc").unwrap_or(false);
    let mut close = false;
    if let Some(r) = f.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
        f.rx = None;
        match r {
            Ok(p) => {
                st.restored = Some(p);
                st.from_file = None;
                return;
            }
            Err(e) => f.error = e,
        }
    }
    egui::Window::new("Ripristina da un file").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.label(f.file.display().to_string());
        if encrypted {
            ui.horizontal(|ui| {
                ui.label("Password di cifratura");
                ui.add(egui::TextEdit::singleline(&mut f.password).password(true).desired_width(220.0));
            });
        }
        if !f.error.is_empty() {
            ui.label(RichText::new(&f.error).color(theme::NEGATIVE));
        }
        ui.horizontal(|ui| {
            let busy = f.rx.is_some();
            if theme::primary_enabled(ui, !busy, icon::CHECK, if busy { "Controllo..." } else { "Ripristina" }).clicked() {
                let (tx, rx) = channel();
                let file = f.file.clone();
                let pw = f.password.clone();
                let name = file.file_stem().map(|s| s.to_string_lossy().trim_end_matches(".gadb").to_string()).unwrap_or_else(|| "archivio".into());
                let out = output_path(&name);
                std::thread::spawn(move || {
                    let r = openfatt_backup::restore_file(&file, Some(&pw), &out).map(|_| out);
                    let _ = tx.send(r);
                });
                f.rx = Some(rx);
                f.error.clear();
            }
            if theme::button(ui, "", "Annulla").clicked() {
                close = true;
            }
        });
    });
    if close {
        st.from_file = None;
    }
}
