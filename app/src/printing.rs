//! Stampe personalizzate dei documenti ed etichette.

use std::collections::BTreeMap;

use openfatt_core::calc;
use openfatt_core::model::{DocKind, Document, LineKind, Party, Product};
use openfatt_core::money::fmt_eur;
use openfatt_pdf::labels::LabelContent;
use openfatt_pdf::freeform::{self, FreeLayout};
use openfatt_pdf::{Image, PrintDoc, PrintStyle, Template};
use openfatt_store::Store;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Impostazioni per un tipo di documento.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct KindPrint {
    /// Titolo stampato al posto di quello predefinito (es. "Offerta" per i preventivi).
    pub title: String,
    /// Mostra i prezzi (es. DDT valorizzato); None = comportamento predefinito.
    pub prices: Option<bool>,
    /// Testo aggiunto in fondo (es. condizioni di vendita).
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PrintSettings {
    /// "classico", "moderno", "essenziale"
    pub template: String,
    /// Colore aziendale (#rrggbb) per il modello moderno.
    pub accent: String,
    pub show_logo: bool,
    pub show_codes: bool,
    pub show_discount: bool,
    pub show_vat: bool,
    pub page_numbers: bool,
    /// Per tipo di documento (codice del tipo).
    pub kinds: BTreeMap<String, KindPrint>,
    pub label_format: String,
    pub label_border: bool,
    pub label_vat_included: bool,
}

impl Default for PrintSettings {
    fn default() -> Self {
        PrintSettings {
            template: "classico".into(),
            accent: "#1d5fbf".into(),
            show_logo: true,
            show_codes: true,
            show_discount: true,
            show_vat: true,
            page_numbers: true,
            kinds: BTreeMap::new(),
            label_format: "a4-3x8".into(),
            label_border: false,
            label_vat_included: true,
        }
    }
}

pub fn settings(store: &Store) -> PrintSettings {
    store.get_setting("print").unwrap_or_default()
}

pub fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    Some((u8::from_str_radix(&h[0..2], 16).ok()?, u8::from_str_radix(&h[2..4], 16).ok()?, u8::from_str_radix(&h[4..6], 16).ok()?))
}

pub fn logo_image(store: &Store) -> Option<Image> {
    store.logo().ok().flatten().and_then(|b| Image::from_bytes(&b).ok())
}

pub fn style(store: &Store, s: &PrintSettings) -> PrintStyle {
    PrintStyle {
        template: match s.template.as_str() {
            "moderno" => Template::Modern,
            "essenziale" => Template::Essential,
            _ => Template::Classic,
        },
        accent: parse_hex(&s.accent).unwrap_or((0x1d, 0x5f, 0xbf)),
        logo: if s.show_logo { logo_image(store) } else { None },
        show_codes: s.show_codes,
        show_discount: s.show_discount,
        show_vat: s.show_vat,
        page_numbers: s.page_numbers,
    }
}

/// Stampa del documento con il modello e le scelte dell'azienda.
pub fn print_doc(store: &Store, doc: &Document, s: &PrintSettings, st: &PrintStyle) -> Result<PrintDoc, String> {
    let company = store.company().map_err(|e| e.to_string())?;
    let totals = calc::compute(doc).map_err(|e| e.0)?;
    let mut p = openfatt_pdf::from_document(&company, doc, &totals);
    p.style = st.clone();
    if let Some(k) = s.kinds.get(doc.kind.code()) {
        if !k.title.trim().is_empty() {
            p.title = k.title.trim().to_string();
        }
        if let Some(prices) = k.prices {
            p.show_prices = prices;
        }
        if !k.note.trim().is_empty() {
            p.notes.push(k.note.trim().to_string());
        }
    }
    Ok(p)
}

/// Modelli di stampa liberi (anche per moduli prestampati).
pub fn layouts(store: &Store) -> Vec<FreeLayout> {
    store.get_setting("print_layouts").unwrap_or_default()
}

pub fn save_layouts(store: &Store, l: &[FreeLayout]) -> Result<(), String> {
    store.set_setting("print_layouts", &l).map_err(|e| e.to_string())
}

/// Modello libero assegnato a un tipo di documento, se c'è.
pub fn layout_for(store: &Store, kind: DocKind) -> Option<FreeLayout> {
    layouts(store).into_iter().find(|l| l.applies_to(kind.code()))
}

/// PDF di un documento con un modello libero.
pub fn free_pdf(store: &Store, l: &FreeLayout, doc: &Document) -> Result<Vec<u8>, String> {
    let s = settings(store);
    let st = style(store, &s);
    let p = print_doc(store, doc, &s, &st)?;
    Ok(freeform::render(l, &[&p], logo_image(store).as_ref()))
}

pub fn document_pdf(store: &Store, doc: &Document) -> Result<Vec<u8>, String> {
    if let Some(l) = layout_for(store, doc.kind) {
        return free_pdf(store, &l, doc);
    }
    let s = settings(store);
    let st = style(store, &s);
    Ok(openfatt_pdf::render(&print_doc(store, doc, &s, &st)?))
}

/// Più documenti in un solo PDF (stampa in blocco).
pub fn documents_pdf(store: &Store, ids: &[i64]) -> Result<Vec<u8>, String> {
    let s = settings(store);
    let st = style(store, &s);
    let mut docs = Vec::new();
    for id in ids {
        let d = store.document(*id).map_err(|e| e.to_string())?;
        docs.push(print_doc(store, &d, &s, &st)?);
    }
    if docs.is_empty() {
        return Err("Nessun documento".into());
    }
    // tutti dello stesso tipo con un modello libero: un solo PDF con quel modello
    let kinds: Vec<DocKind> = ids.iter().filter_map(|id| store.document(*id).ok().map(|d| d.kind)).collect();
    if let Some(l) = kinds.first().and_then(|k| layout_for(store, *k)) {
        if kinds.iter().all(|k| l.applies_to(k.code())) {
            let refs: Vec<&PrintDoc> = docs.iter().collect();
            return Ok(freeform::render(&l, &refs, logo_image(store).as_ref()));
        }
    }
    Ok(openfatt_pdf::render_many(&docs))
}

// ----------------------------------------------------------------------
// Etichette
// ----------------------------------------------------------------------

/// Un'etichetta da stampare con la sua quantità.
#[derive(Debug, Clone)]
pub struct LabelItem {
    pub label: String,
    pub content: LabelContent,
    pub qty: u32,
}

fn vat_rate(store: &Store, p: &Product) -> Decimal {
    let vats = store.vat_codes().unwrap_or_default();
    p.vat_id.and_then(|id| vats.iter().find(|v| v.id == id)).map(|v| v.rate).unwrap_or_default()
}

pub fn product_label(store: &Store, p: &Product, s: &PrintSettings) -> LabelContent {
    let price = if s.label_vat_included { (p.price * (Decimal::ONE + vat_rate(store, p) / Decimal::ONE_HUNDRED)).round_dp(2) } else { p.price };
    let code = if p.barcode.trim().is_empty() { p.code.trim().to_string() } else { p.barcode.trim().to_string() };
    LabelContent::Product {
        title: p.description.clone(),
        code: p.code.clone(),
        price: (!price.is_zero()).then(|| format!("€ {}", fmt_eur(price))),
        barcode: (!code.is_empty()).then_some(code),
        note: String::new(),
    }
}

pub fn product_items(store: &Store, products: &[Product]) -> Vec<LabelItem> {
    let s = settings(store);
    products.iter().map(|p| LabelItem { label: format!("{} - {}", p.code, p.description), content: product_label(store, p, &s), qty: 1 }).collect()
}

/// Etichette degli articoli di un documento (es. arrivo merce): quantità dalle righe.
pub fn document_items(store: &Store, doc: &Document) -> Vec<LabelItem> {
    use rust_decimal::prelude::ToPrimitive;
    let s = settings(store);
    let mut out = Vec::new();
    for l in &doc.lines {
        if l.kind != LineKind::Item {
            continue;
        }
        let Some(pid) = l.product_id else { continue };
        let Ok(Some(p)) = store.product(pid) else { continue };
        if p.is_service {
            continue;
        }
        let mut content = product_label(store, &p, &s);
        if let LabelContent::Product { note, barcode, .. } = &mut content {
            let mut extra = Vec::new();
            if !l.variant.is_empty() {
                extra.push(l.variant.clone());
                if let Some(vb) = p.variants.barcodes.iter().find(|b| b.variant == l.variant) {
                    *barcode = Some(vb.barcode.clone());
                }
            }
            if !l.lot.is_empty() {
                extra.push(format!("lotto {}", l.lot));
            }
            *note = extra.join(" ");
        }
        let qty = l.qty.ceil().to_u32().unwrap_or(1).clamp(1, 999);
        out.push(LabelItem { label: format!("{} - {}{}", p.code, p.description, if l.variant.is_empty() { String::new() } else { format!(" ({})", l.variant) }), content, qty });
    }
    out
}

pub fn address_item(p: &Party) -> LabelItem {
    let mut lines = vec![p.display_name(), p.full_address(), p.city_line()];
    if !p.country.is_empty() && p.country != "IT" {
        lines.push(p.country.clone());
    }
    lines.retain(|l| !l.trim().is_empty());
    LabelItem { label: p.display_name(), content: LabelContent::Address { heading: "Spett.le".into(), lines }, qty: 1 }
}

/// Etichetta di spedizione di un DDT o di una fattura accompagnatoria (una per collo).
pub fn shipping_item(store: &Store, doc: &Document) -> LabelItem {
    let c = store.company().unwrap_or_default();
    let p = &doc.party;
    let mut recipient = vec![p.display_name()];
    let dest: Vec<String> = doc.destination.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if dest.is_empty() {
        recipient.push(p.full_address());
        recipient.push(p.city_line());
    } else {
        recipient.extend(dest);
    }
    recipient.retain(|l| !l.trim().is_empty());
    let packages = doc.transport.as_ref().and_then(|t| t.packages).unwrap_or(1).max(1);
    let mut info = vec![format!("{} n. {} del {}", doc.kind.label(), doc.number_label(), doc.date.format("%d/%m/%Y"))];
    if let Some(t) = &doc.transport {
        if let Some(w) = t.gross_weight.filter(|w| !w.is_zero()) {
            info.push(format!("Peso lordo {} kg", openfatt_core::money::fmt_it_trim(w, 0, 3)));
        }
    }
    LabelItem {
        label: format!("Spedizione {} {}", doc.kind.label(), doc.number_label()),
        content: LabelContent::Shipping { sender: vec![c.display_name(), format!("{} {}", c.address, c.civic).trim().to_string(), format!("{} {} {}", c.zip, c.city, c.province).trim().to_string()], recipient, info },
        qty: packages,
    }
}

/// PDF delle etichette: ogni elemento ripetuto per la sua quantità; per le spedizioni si
/// indica anche "collo n di N".
pub fn labels_pdf(items: &[LabelItem], format: &str, skip: usize, border: bool) -> Vec<u8> {
    let mut list = Vec::new();
    for it in items {
        for n in 0..it.qty {
            let mut c = it.content.clone();
            if let LabelContent::Shipping { info, .. } = &mut c {
                info.push(format!("Collo {} di {}", n + 1, it.qty));
            }
            list.push(c);
        }
    }
    openfatt_pdf::labels::render_labels(&openfatt_pdf::labels::format_by_id(format), &list, skip, border)
}

#[allow(dead_code)]
pub fn is_shipping_doc(k: DocKind) -> bool {
    matches!(k, DocKind::DeliveryNote | DocKind::AccompanyingInvoice)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_logo() -> Vec<u8> {
        // 3x2 RGBA con trasparenza
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 3, 2);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[29, 95, 191, 255, 29, 95, 191, 255, 0, 0, 0, 0, 20, 20, 20, 255, 20, 20, 20, 255, 0, 0, 0, 0]).unwrap();
        }
        out
    }

    #[test]
    fn stampe_personalizzate_e_etichette() {
        let s = Store::open_in_memory().unwrap();
        crate::demo::populate(&s).unwrap();
        s.set_logo(Some(&png_logo())).unwrap();
        let mut ps = settings(&s);
        ps.template = "moderno".into();
        ps.accent = "#0a7d4b".into();
        ps.show_codes = false;
        ps.kinds.insert("DDT".into(), KindPrint { title: "Documento di trasporto".into(), prices: Some(true), note: "Merce viaggiante a rischio del destinatario.".into() });
        s.set_setting("print", &ps).unwrap();
        let docs = s.documents(&openfatt_store::DocFilter::default()).unwrap();
        let d = docs
            .iter()
            .filter(|d| d.kind == DocKind::DeliveryNote && d.number.is_some())
            .map(|d| s.document(d.id).unwrap())
            .find(|d| d.transport.as_ref().and_then(|t| t.packages) == Some(4))
            .expect("DDT con 4 colli");
        let st = style(&s, &ps);
        assert!(st.logo.is_some());
        assert_eq!(st.accent, (0x0a, 0x7d, 0x4b));
        let pd = print_doc(&s, &d, &ps, &st).unwrap();
        assert_eq!(pd.title, "Documento di trasporto");
        assert!(pd.show_prices);
        assert!(pd.notes.iter().any(|n| n.contains("rischio")));
        let pdf = document_pdf(&s, &d).unwrap();
        let txt = String::from_utf8_lossy(&pdf);
        assert!(txt.contains("/Subtype /Image") && txt.contains("/SMask"));
        let ids: Vec<i64> = docs.iter().filter(|d| d.number.is_some()).map(|d| d.id).take(3).collect();
        let all = documents_pdf(&s, &ids).unwrap();
        assert!(String::from_utf8_lossy(&all).matches("/Type /Page ").count() >= 3);
        // etichette: articoli del DDT, indirizzo e spedizione
        let items = document_items(&s, &d);
        assert!(!items.is_empty());
        let ship = shipping_item(&s, &d);
        assert_eq!(ship.qty, 4); // 4 colli nei dati dimostrativi
        let addr = address_item(&d.party);
        let mut all_items = items.clone();
        all_items.push(ship);
        all_items.push(addr);
        let lp = labels_pdf(&all_items, "a4-3x8", 2, true);
        assert!(lp.starts_with(b"%PDF"));
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/pdf-samples");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("ddt-moderno.pdf"), &pdf).unwrap();
        std::fs::write(out.join("etichette-ddt.pdf"), &lp).unwrap();
        // modello classico e essenziale per confronto
        for t in ["classico", "essenziale"] {
            ps.template = t.into();
            ps.show_codes = true;
            s.set_setting("print", &ps).unwrap();
            let inv = docs.iter().find(|d| d.kind == DocKind::Invoice && d.number.is_some()).unwrap();
            std::fs::write(out.join(format!("fattura-{}.pdf", t)), document_pdf(&s, &s.document(inv.id).unwrap()).unwrap()).unwrap();
        }
        std::fs::write(out.join("etichette-rotolo.pdf"), labels_pdf(&product_items(&s, &s.products("", false).unwrap().into_iter().take(3).collect::<Vec<_>>()), "roll-50x30", 0, false)).unwrap();
    }
}
