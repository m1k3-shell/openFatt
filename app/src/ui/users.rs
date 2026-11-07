//! Utenti, permessi e postazioni collegate; finestra di accesso; dove sono i dati.

use eframe::egui::{self, RichText};
use openfatt_store::{Access, Area, User};

use super::theme::{self, icon, Tone};
use super::widgets::{GRAY, ORANGE};
use super::Ctx;

#[derive(Default)]
pub struct State {
    editing: Option<(User, String, String)>,
}

pub fn settings_tab(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    ui.label(theme::medium("Dove sono i dati"));
    egui::Grid::new("data_paths").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        ui.label("Archivio aperto");
        ui.horizontal(|ui| {
            ui.label(cx.store.path().display().to_string());
            ui.label(RichText::new(format!("({:.1} MB{})", cx.store.file_size() as f64 / 1_048_576.0, if cx.store.is_network() { ", in rete" } else { "" })).color(GRAY));
            if ui.small_button("Apri la cartella").clicked() {
                if let Some(d) = cx.store.path().parent() {
                    crate::services::open_path(d);
                }
            }
        });
        ui.end_row();
        ui.label("Archivi nuovi");
        ui.label(crate::config::documents_dir().display().to_string());
        ui.end_row();
        ui.label("Copie di sicurezza automatiche");
        ui.horizontal(|ui| {
            ui.label(crate::config::backup_dir().display().to_string());
            if ui.small_button("Apri").clicked() {
                crate::services::open_path(&crate::config::backup_dir());
            }
        });
        ui.end_row();
        ui.label("Preferenze del programma");
        ui.label(crate::config::config_dir().join("config.json").display().to_string());
        ui.end_row();
        ui.label("Password");
        ui.label("Gestore credenziali di Windows (non sono nell'archivio)");
        ui.end_row();
    });
    ui.label(RichText::new("L'archivio è un unico file SQLite: per spostarlo basta copiarlo (a programma chiuso) e riaprirlo. Ogni giorno alla chiusura se ne fa una copia; si tengono le ultime 15.").small().color(GRAY));
    ui.add_space(10.0);
    theme::rule(ui);
    ui.add_space(6.0);
    ui.label(theme::medium("Postazioni collegate a questo archivio"));
    let sessions = cx.store.active_sessions().unwrap_or_default();
    // una riga per postazione e utente (un programma chiuso male resta per qualche minuto)
    let mut seen: Vec<(String, String, usize, String)> = Vec::new();
    for s in &sessions {
        let user = if s.username.is_empty() { "senza utente".to_string() } else { s.username.clone() };
        match seen.iter_mut().find(|x| x.0 == s.host && x.1 == user) {
            Some(x) => x.2 += 1,
            None => seen.push((s.host.clone(), user, 1, s.started_at.chars().take(16).collect())),
        }
    }
    for (host, user, n, since) in &seen {
        ui.label(format!("{} · {} · dal {}{}", host, user, since, if *n > 1 { format!(" ({} finestre)", n) } else { String::new() }));
    }
    ui.label(RichText::new("Per lavorare in più postazioni si mette l'archivio in una cartella condivisa (\\\\server\\dati o un'unità di rete) e lo si apre da ogni computer. In rete il programma usa il registro classico di SQLite, compatibile con la condivisione di file; chi apre un documento già aperto altrove lo vede in sola lettura. Per molte postazioni contemporanee (oltre 5-6) conviene un disco del server veloce e una rete cablata.").small().color(GRAY));
    if !cx.store.is_network() && sessions.len() > 1 {
        ui.label(RichText::new("Attenzione: più postazioni su un archivio che il programma non riconosce come cartella di rete. Avviare con la variabile OPENFATT_NETWORK=1 se il file è su un disco condiviso.").small().color(ORANGE));
    }
    ui.add_space(10.0);
    theme::rule(ui);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(theme::medium("Utenti e permessi"));
        if theme::button(ui, icon::USER_PLUS, "Nuovo utente").clicked() {
            let first = cx.store.users().map(|u| u.is_empty()).unwrap_or(true);
            st.editing = Some((User { admin: first, ..Default::default() }, String::new(), String::new()));
        }
    });
    let users = cx.store.users().unwrap_or_default();
    if users.is_empty() {
        ui.label(RichText::new("Nessun utente: l'archivio si apre senza accesso. Creando il primo utente (amministratore) all'apertura verranno chiesti nome e password, e ogni operazione nel registro sarà firmata con l'utente.").color(GRAY));
    }
    for u in &users {
        ui.horizontal(|ui| {
            ui.label(theme::medium(&u.username));
            ui.label(RichText::new(&u.full_name).color(GRAY));
            if u.admin {
                theme::pill(ui, "Amministratore", Tone::Positive);
            }
            if !u.active {
                theme::pill(ui, "Non attivo", Tone::Negative);
            }
            if ui.small_button("Modifica").clicked() {
                st.editing = Some((u.clone(), String::new(), String::new()));
            }
        });
    }
    edit_window(ui, cx, st);
}

fn edit_window(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let Some((u, pw, pw2)) = &mut st.editing else { return };
    let mut close = false;
    egui::Window::new(if u.id == 0 { "Nuovo utente" } else { "Utente" }).collapsible(false).default_width(520.0).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
        egui::Grid::new("user_form").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Nome utente");
            ui.add(egui::TextEdit::singleline(&mut u.username).desired_width(180.0));
            ui.end_row();
            ui.label("Nome e cognome");
            ui.add(egui::TextEdit::singleline(&mut u.full_name).desired_width(260.0));
            ui.end_row();
            ui.label(if u.id == 0 { "Password" } else { "Nuova password" });
            ui.add(egui::TextEdit::singleline(pw).password(true).desired_width(180.0).hint_text(if u.id == 0 { "almeno 8 caratteri" } else { "vuoto = invariata" }));
            ui.end_row();
            ui.label("Ripeti la password");
            ui.add(egui::TextEdit::singleline(pw2).password(true).desired_width(180.0));
            ui.end_row();
            ui.label("");
            ui.horizontal(|ui| {
                ui.checkbox(&mut u.admin, "Amministratore (tutti i permessi)");
                ui.checkbox(&mut u.active, "Attivo");
            });
            ui.end_row();
        });
        if !u.admin {
            ui.add_space(4.0);
            egui::Grid::new("perms").striped(true).num_columns(4).spacing([12.0, 4.0]).show(ui, |ui| {
                ui.label(RichText::new("Area").strong());
                ui.label(RichText::new("Nessuno").strong());
                ui.label(RichText::new("Lettura").strong());
                ui.label(RichText::new("Modifica").strong());
                ui.end_row();
                for a in Area::ALL {
                    let pos = match u.permissions.iter().position(|(x, _)| *x == a) {
                        Some(i) => i,
                        None => {
                            u.permissions.push((a, Access::None));
                            u.permissions.len() - 1
                        }
                    };
                    let p = &mut u.permissions[pos].1;
                    ui.label(a.label());
                    ui.radio_value(p, Access::None, "");
                    ui.radio_value(p, Access::Read, "");
                    ui.radio_value(p, Access::Write, "");
                    ui.end_row();
                }
            });
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if theme::primary(ui, icon::CHECK, "Salva").clicked() {
                if pw != pw2 {
                    cx.msgs.err("Le due password non coincidono");
                } else {
                    let np = if pw.is_empty() { None } else { Some(pw.as_str()) };
                    if cx.msgs.check(cx.store.save_user(u, np), "Utente salvato").is_some() {
                        if cx.store.current_user().is_empty() {
                            let _ = cx.store.login(&u.username, pw);
                        }
                        close = true;
                    }
                }
            }
            if u.id != 0 && theme::button(ui, icon::TRASH, "Elimina").clicked() && cx.msgs.check(cx.store.delete_user(u.id), "Utente eliminato").is_some() {
                close = true;
            }
            if theme::button(ui, icon::X, "Annulla").clicked() {
                close = true;
            }
        });
    });
    if close {
        st.editing = None;
    }
}

/// Finestra di accesso. Restituisce l'utente collegato quando nome e password sono corretti.
pub fn login_window(ctx: &egui::Context, store: &openfatt_store::Store, name: &mut String, pw: &mut String, error: &mut String) -> Option<User> {
    let mut out = None;
    egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::GROUND)).show(ctx, |_| {});
    egui::Window::new("Accesso").collapsible(false).resizable(false).title_bar(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.horizontal(|ui| {
            theme::mark(ui, 32.0);
            ui.vertical(|ui| {
                ui.label(theme::bold("openFatt").size(20.0));
                ui.label(RichText::new(store.company().map(|c| c.display_name()).unwrap_or_default()).color(GRAY));
            });
        });
        ui.add_space(10.0);
        ui.label("Utente");
        let r1 = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
        if name.is_empty() {
            r1.request_focus();
        }
        ui.label("Password");
        let r = ui.add(egui::TextEdit::singleline(pw).password(true).desired_width(f32::INFINITY));
        if !error.is_empty() {
            ui.label(RichText::new(error.as_str()).color(theme::NEGATIVE));
        }
        ui.add_space(6.0);
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if theme::primary(ui, icon::SIGN_IN, "Accedi").clicked() || enter {
            match store.login(name, pw) {
                Ok(u) => {
                    pw.clear();
                    error.clear();
                    out = Some(u);
                }
                Err(e) => {
                    *error = e.to_string();
                    pw.clear();
                }
            }
        }
    });
    out
}
