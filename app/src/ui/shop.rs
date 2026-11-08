//! Negozio online: scarico degli ordini e pubblicazione di prezzi e giacenze degli articoli.

use std::sync::mpsc::{channel, Receiver};

use eframe::egui::{self, RichText};
use openfatt_interop::shop::{ShopKind, ShopProduct};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::theme::{self, icon};
use super::widgets::GRAY;
use super::{Ctx, Nav};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct ShopSettings {
    pub kind: ShopKind,
    /// WooCommerce: indirizzo del sito (https://negozio.example); Shopify: nome-negozio.myshopify.com
    pub url: String,
    /// WooCommerce: consumer key; Shopify: non usato
    pub key: String,
    /// Stato degli ordini da scaricare (WooCommerce: processing; Shopify: any)
    pub status: String,
    pub publish_stock: bool,
}

enum Job {
    Orders(Result<String, String>),
    Published(usize, Vec<String>),
}

#[derive(Default)]
pub struct State {
    cfg: Option<ShopSettings>,
    secret: String,
    busy: Option<Receiver<Job>>,
    log: Vec<String>,
}

const KEYRING: &str = "gestionale-aperto-shop";

fn secret_for(cfg: &ShopSettings) -> Option<String> {
    keyring::Entry::new(KEYRING, &cfg.url).ok()?.get_password().ok()
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(30)).build()
}

fn base(cfg: &ShopSettings) -> String {
    let u = cfg.url.trim().trim_end_matches('/');
    if u.starts_with("http") {
        u.to_string()
    } else {
        format!("https://{}", u)
    }
}

fn fetch_orders(cfg: &ShopSettings, secret: &str) -> Result<String, String> {
    let a = agent();
    let resp = match cfg.kind {
        ShopKind::WooCommerce => {
            let status = if cfg.status.is_empty() { "processing" } else { &cfg.status };
            let url = format!("{}/wp-json/wc/v3/orders?per_page=50&status={}", base(cfg), status);
            let auth = base64_basic(&cfg.key, secret);
            a.get(&url).set("Authorization", &auth).call()
        }
        ShopKind::Shopify => {
            let status = if cfg.status.is_empty() { "any" } else { &cfg.status };
            let url = format!("{}/admin/api/2024-10/orders.json?status={}&limit=50", base(cfg), status);
            a.get(&url).set("X-Shopify-Access-Token", secret).call()
        }
    };
    match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, r)) => Err(format!("Il negozio ha risposto {}: {}", code, r.into_string().unwrap_or_default().chars().take(200).collect::<String>())),
        Err(e) => Err(format!("Collegamento al negozio non riuscito: {}", e)),
    }
}

fn base64_basic(user: &str, pass: &str) -> String {
    use std::fmt::Write;
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let input = format!("{}:{}", user, pass);
    let b = input.as_bytes();
    let mut out = String::from("Basic ");
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                let _ = out.write_char(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn publish(cfg: &ShopSettings, secret: &str, items: &[ShopProduct]) -> (usize, Vec<String>) {
    let a = agent();
    let mut ok = 0;
    let mut errors = Vec::new();
    for p in items {
        let r = match cfg.kind {
            ShopKind::WooCommerce => a.post(&format!("{}/wp-json/wc/v3/products", base(cfg))).set("Authorization", &base64_basic(&cfg.key, secret)).set("Content-Type", "application/json").send_string(&openfatt_interop::shop::woocommerce_product_json(p)),
            ShopKind::Shopify => a.post(&format!("{}/admin/api/2024-10/products.json", base(cfg))).set("X-Shopify-Access-Token", secret).set("Content-Type", "application/json").send_string(&openfatt_interop::shop::shopify_product_json(p)),
        };
        match r {
            Ok(_) => ok += 1,
            Err(ureq::Error::Status(code, _)) => errors.push(format!("{}: risposta {}", p.sku, code)),
            Err(e) => {
                errors.push(format!("{}: {}", p.sku, e));
                break;
            }
        }
    }
    (ok, errors)
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    theme::title_row(ui, &["Vendite"], "Negozio online", None, |_| {});
    let cfg = st.cfg.get_or_insert_with(|| cx.store.get_setting("shop").unwrap_or_default()).clone();
    // esito dei lavori in corso
    if let Some(rx) = &st.busy {
        match rx.try_recv() {
            Ok(Job::Orders(r)) => {
                st.busy = None;
                match r {
                    Ok(json) => import_json(cx, st, cfg.kind, &json),
                    Err(e) => cx.msgs.err(e),
                }
            }
            Ok(Job::Published(n, errs)) => {
                st.busy = None;
                st.log.push(format!("Pubblicati {} articoli{}", n, if errs.is_empty() { String::new() } else { format!(", {} errori", errs.len()) }));
                st.log.extend(errs);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            }
            Err(_) => st.busy = None,
        }
    }
    ui.label(RichText::new("Si usano le API ufficiali del negozio: per WooCommerce una chiave REST (Opzioni > Avanzate > REST API) con permessi di lettura e scrittura, per Shopify un token di accesso di un'app personalizzata. Il segreto è salvato nel Gestore credenziali di Windows.").small().color(GRAY));
    ui.add_space(6.0);
    let c = st.cfg.as_mut().unwrap();
    egui::Grid::new("shop_cfg").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Piattaforma");
        ui.horizontal(|ui| {
            ui.radio_value(&mut c.kind, ShopKind::WooCommerce, "WooCommerce");
            ui.radio_value(&mut c.kind, ShopKind::Shopify, "Shopify");
        });
        ui.end_row();
        ui.label(if c.kind == ShopKind::Shopify { "Negozio" } else { "Indirizzo del sito" });
        ui.add(egui::TextEdit::singleline(&mut c.url).desired_width(320.0).hint_text(if c.kind == ShopKind::Shopify { "nome.myshopify.com" } else { "https://negozio.example" }));
        ui.end_row();
        if c.kind == ShopKind::WooCommerce {
            ui.label("Consumer key");
            ui.add(egui::TextEdit::singleline(&mut c.key).desired_width(320.0));
            ui.end_row();
        }
        ui.label(if c.kind == ShopKind::Shopify { "Token di accesso" } else { "Consumer secret" });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut st.secret).password(true).desired_width(240.0).hint_text(if secret_for(c).is_some() { "salvato" } else { "" }));
            if ui.button("Salva").clicked() {
                if !st.secret.is_empty() {
                    match keyring::Entry::new(KEYRING, &c.url).and_then(|e| e.set_password(&st.secret)) {
                        Ok(_) => st.secret.clear(),
                        Err(e) => cx.msgs.err(e.to_string()),
                    }
                }
                cx.msgs.check(cx.store.set_setting("shop", &*c), "Impostazioni del negozio salvate");
            }
        });
        ui.end_row();
        ui.label("Stato degli ordini");
        ui.add(egui::TextEdit::singleline(&mut c.status).desired_width(120.0).hint_text(if c.kind == ShopKind::Shopify { "any" } else { "processing" }));
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut c.publish_stock, "Pubblica anche le giacenze");
        ui.end_row();
    });
    ui.add_space(8.0);
    let c = st.cfg.clone().unwrap_or_default();
    let c = &c;
    let busy = st.busy.is_some();
    ui.horizontal(|ui| {
        if theme::primary_enabled(ui, !busy && !c.url.is_empty(), icon::DOWNLOAD_SIMPLE, if busy { "Collegamento in corso..." } else { "Scarica gli ordini" }).clicked() {
            match secret_for(c) {
                Some(secret) => {
                    let (tx, rx) = channel();
                    let cc = c.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send(Job::Orders(fetch_orders(&cc, &secret)));
                    });
                    st.busy = Some(rx);
                }
                None => cx.msgs.err("Salvare prima il segreto di accesso al negozio"),
            }
        }
        if theme::button_enabled(ui, !busy && !c.url.is_empty(), icon::UPLOAD_SIMPLE, "Pubblica gli articoli").on_hover_text("Articoli con \"Pubblicato sul negozio online\" nella scheda").clicked() {
            let items = online_products(cx);
            match (secret_for(c), items.is_empty()) {
                (_, true) => cx.msgs.warn("Nessun articolo da pubblicare: spuntare \"Negozio online\" nelle schede degli articoli"),
                (None, _) => cx.msgs.err("Salvare prima il segreto di accesso al negozio"),
                (Some(secret), false) => {
                    let (tx, rx) = channel();
                    let cc = c.clone();
                    std::thread::spawn(move || {
                        let (n, e) = publish(&cc, &secret, &items);
                        let _ = tx.send(Job::Published(n, e));
                    });
                    st.busy = Some(rx);
                }
            }
        }
        if theme::button(ui, icon::FILE_ARROW_UP, "Importa ordini da file JSON...").on_hover_text("Risposta dell'API salvata su file").clicked() {
            if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() {
                match std::fs::read_to_string(&p) {
                    Ok(json) => {
                        let k = c.kind;
                        import_json(cx, st, k, &json);
                    }
                    Err(e) => cx.msgs.err(e.to_string()),
                }
            }
        }
    });
    ui.add_space(8.0);
    for l in st.log.iter().rev().take(20) {
        ui.label(RichText::new(l).color(GRAY));
    }
}

fn online_products(cx: &mut Ctx) -> Vec<ShopProduct> {
    let vats = cx.store.vat_codes().unwrap_or_default();
    let publish_stock = cx.store.get_setting::<ShopSettings>("shop").map(|s| s.publish_stock).unwrap_or(false);
    cx.store
        .products("", false)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.online)
        .map(|p| {
            let rate = p.vat_id.and_then(|id| vats.iter().find(|v| v.id == id)).map(|v| v.rate).unwrap_or(Decimal::from(22));
            ShopProduct {
                sku: p.code.clone(),
                name: p.description.clone(),
                gross_price: openfatt_core::money::round2(p.price * (Decimal::ONE + rate / Decimal::ONE_HUNDRED)),
                stock: if publish_stock && p.manage_stock { cx.store.stock_of(p.id).ok() } else { None },
                description: p.notes.clone(),
            }
        })
        .collect()
}

fn import_json(cx: &mut Ctx, st: &mut State, kind: ShopKind, json: &str) {
    let parsed = match kind {
        ShopKind::WooCommerce => openfatt_interop::shop::parse_woocommerce(json),
        ShopKind::Shopify => openfatt_interop::shop::parse_shopify(json),
    };
    match parsed {
        Err(e) => cx.msgs.err(e),
        Ok(orders) => {
            if let Some(r) = cx.msgs.check(cx.store.import_shop_orders(kind, &orders), "") {
                let msg = format!("{} ordini importati come ordini cliente in bozza, {} già presenti, {} clienti nuovi", r.created.len(), r.already, r.new_customers);
                cx.msgs.ok(msg.clone());
                st.log.push(msg);
                if !r.unknown_skus.is_empty() {
                    st.log.push(format!("Codici non presenti in anagrafica: {}", r.unknown_skus.join(", ")));
                }
                if r.created.len() == 1 {
                    cx.nav.push(Nav::OpenDocument(r.created[0]));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn basic_auth_header() {
        assert_eq!(super::base64_basic("ck_1", "cs_2"), "Basic Y2tfMTpjc18y");
        assert_eq!(super::base64_basic("a", "b"), "Basic YTpi");
    }
}
