//! Etichette: articoli con prezzo e codice a barre, indirizzi, spedizioni.
//! Fogli A4 con più etichette (formati comuni) oppure rotoli per etichettatrici
//! (una etichetta per pagina della misura dell'etichetta).

use crate::barcode;
use crate::writer::{text_width, wrap, Align, Pdf};

pub const MM: f32 = 2.834_646;

#[derive(Debug, Clone, PartialEq)]
pub struct LabelFormat {
    pub id: &'static str,
    pub name: &'static str,
    /// Pagina in mm.
    pub page_w: f32,
    pub page_h: f32,
    pub cols: usize,
    pub rows: usize,
    pub label_w: f32,
    pub label_h: f32,
    pub left: f32,
    pub top: f32,
    pub gap_x: f32,
    pub gap_y: f32,
}

impl LabelFormat {
    pub fn per_page(&self) -> usize {
        self.cols * self.rows
    }
    pub fn is_roll(&self) -> bool {
        self.per_page() == 1
    }
}

#[allow(clippy::too_many_arguments)]
const fn sheet(id: &'static str, name: &'static str, cols: usize, rows: usize, w: f32, h: f32, left: f32, top: f32, gap_x: f32, gap_y: f32) -> LabelFormat {
    LabelFormat { id, name, page_w: 210.0, page_h: 297.0, cols, rows, label_w: w, label_h: h, left, top, gap_x, gap_y }
}

const fn roll(id: &'static str, name: &'static str, w: f32, h: f32) -> LabelFormat {
    LabelFormat { id, name, page_w: w, page_h: h, cols: 1, rows: 1, label_w: w, label_h: h, left: 0.0, top: 0.0, gap_x: 0.0, gap_y: 0.0 }
}

/// Formati disponibili: fogli A4 dei tipi più diffusi e rotoli per etichettatrici.
pub fn formats() -> Vec<LabelFormat> {
    vec![
        sheet("a4-3x8", "A4, 24 etichette 70 x 37 mm", 3, 8, 70.0, 37.0, 0.0, 0.5, 0.0, 0.0),
        sheet("a4-3x7", "A4, 21 etichette 70 x 42,3 mm", 3, 7, 70.0, 42.3, 0.0, 0.45, 0.0, 0.0),
        sheet("a4-3x7-r", "A4, 21 etichette 63,5 x 38,1 mm (bordi arrotondati)", 3, 7, 63.5, 38.1, 7.2, 15.15, 2.5, 0.0),
        sheet("a4-2x7", "A4, 14 etichette 99,1 x 38,1 mm", 2, 7, 99.1, 38.1, 4.65, 15.15, 2.5, 0.0),
        sheet("a4-4x10", "A4, 40 etichette 48,5 x 25,4 mm", 4, 10, 48.5, 25.4, 8.0, 21.5, 0.0, 0.0),
        sheet("a4-2x4", "A4, 8 etichette 105 x 74 mm (spedizioni)", 2, 4, 105.0, 74.0, 0.0, 0.5, 0.0, 0.0),
        roll("roll-50x30", "Etichettatrice, 50 x 30 mm", 50.0, 30.0),
        roll("roll-62x29", "Etichettatrice, 62 x 29 mm", 62.0, 29.0),
        roll("roll-100x150", "Etichettatrice, 100 x 150 mm (spedizioni)", 100.0, 150.0),
    ]
}

pub fn format_by_id(id: &str) -> LabelFormat {
    formats().into_iter().find(|f| f.id == id).unwrap_or_else(|| formats().remove(0))
}

#[derive(Debug, Clone, PartialEq)]
pub enum LabelContent {
    Product { title: String, code: String, price: Option<String>, barcode: Option<String>, note: String },
    Address { heading: String, lines: Vec<String> },
    Shipping { sender: Vec<String>, recipient: Vec<String>, info: Vec<String> },
}

fn fit_size(s: &str, w: f32, max: f32, bold: bool) -> f32 {
    let mut size = max;
    while size > 5.0 && text_width(s, size, bold) > w {
        size -= 0.5;
    }
    size
}

fn draw_label(pdf: &mut Pdf, c: &LabelContent, x: f32, y: f32, w: f32, h: f32) {
    let pad = (2.5 * MM).min(h * 0.08);
    let (ix, iy, iw, ih) = (x + pad, y + pad, w - 2.0 * pad, h - 2.0 * pad);
    match c {
        LabelContent::Product { title, code, price, barcode: bc, note } => {
            let base = (ih / 6.0).clamp(5.5, 11.0);
            let mut cy = iy + base;
            let lines = wrap(title, iw, base, true);
            for l in lines.iter().take(2) {
                pdf.text(ix, cy, base, true, l, Align::Left);
                cy += base * 1.15;
            }
            let small = (base * 0.75).max(5.0);
            let code_line = if note.is_empty() { code.clone() } else { format!("{}  {}", code, note) };
            pdf.text(ix, cy, small, false, &code_line, Align::Left);
            cy += small * 0.6;
            let bar = bc.as_deref().and_then(barcode::auto);
            let price_size = (ih / 4.2).clamp(7.0, 22.0);
            let bottom = iy + ih;
            match (bar, price) {
                (Some(b), Some(p)) => {
                    let pw = text_width(p, price_size, true) + 4.0;
                    let bw = (iw - pw).max(iw * 0.55);
                    let bh = (bottom - cy).max(ih * 0.3);
                    barcode::draw(pdf, &b, ix - pad * 0.6, bottom - bh, bw, bh, (bh / 5.5).clamp(4.5, 8.0));
                    let ps = fit_size(p, iw - bw, price_size, true);
                    pdf.text(ix + iw, bottom - 1.0, ps, true, p, Align::Right);
                }
                (Some(b), None) => {
                    let bh = (bottom - cy).max(ih * 0.3);
                    barcode::draw(pdf, &b, ix - pad * 0.6, bottom - bh, iw + pad * 1.2, bh, (bh / 5.5).clamp(4.5, 8.0));
                }
                (None, Some(p)) => {
                    pdf.text(ix + iw, bottom - 1.0, fit_size(p, iw, price_size * 1.3, true), true, p, Align::Right);
                }
                (None, None) => {}
            }
        }
        LabelContent::Address { heading, lines } => {
            let n = lines.len() + usize::from(!heading.is_empty());
            let size = (ih / (n.max(3) as f32 * 1.3)).clamp(6.0, 12.0);
            let mut cy = iy + size;
            if !heading.is_empty() {
                pdf.text(ix, cy, size * 0.7, true, heading, Align::Left);
                cy += size * 1.1;
            }
            for (i, l) in lines.iter().enumerate() {
                let s = fit_size(l, iw, size, i == 0);
                pdf.text(ix, cy, s, i == 0, l, Align::Left);
                cy += size * 1.3;
            }
        }
        LabelContent::Shipping { sender, recipient, info } => {
            let small = (ih / 22.0).clamp(5.5, 8.5);
            let mut cy = iy + small;
            pdf.text(ix, cy, small * 0.9, true, "MITTENTE", Align::Left);
            cy += small * 1.2;
            for l in sender.iter().take(4) {
                pdf.text(ix, cy, small, false, l, Align::Left);
                cy += small * 1.2;
            }
            cy += small * 0.6;
            pdf.line(ix, cy - small * 0.4, ix + iw, cy - small * 0.4, 0.6);
            let big = (ih / 11.0).clamp(8.0, 16.0);
            cy += small;
            pdf.text(ix, cy, small * 0.9, true, "DESTINATARIO", Align::Left);
            cy += big * 1.1;
            for (i, l) in recipient.iter().take(5).enumerate() {
                let s = fit_size(l, iw, big, i == 0);
                pdf.text(ix, cy, s, i == 0, l, Align::Left);
                cy += big * 1.25;
            }
            let mut by = iy + ih;
            for l in info.iter().rev() {
                pdf.text(ix, by, small * 1.1, true, l, Align::Left);
                by -= small * 1.5;
            }
        }
    }
}

/// Etichette in PDF. `skip` salta le prime posizioni del primo foglio (etichette già usate).
pub fn render_labels(f: &LabelFormat, labels: &[LabelContent], skip: usize, border: bool) -> Vec<u8> {
    let mut pdf = Pdf::with_size("Etichette", f.page_w * MM, f.page_h * MM);
    let per = f.per_page();
    let skip = if f.is_roll() { 0 } else { skip % per };
    for (n, c) in labels.iter().enumerate() {
        let pos = n + skip;
        if pos > 0 && pos % per == 0 {
            pdf.new_page();
        }
        let slot = pos % per;
        let (col, row) = (slot % f.cols, slot / f.cols);
        let x = (f.left + col as f32 * (f.label_w + f.gap_x)) * MM;
        let y = (f.top + row as f32 * (f.label_h + f.gap_y)) * MM;
        let (w, h) = (f.label_w * MM, f.label_h * MM);
        if border {
            pdf.rect(x, y, w, h, None, true);
        }
        draw_label(&mut pdf, c, x, y, w, h);
    }
    pdf.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i_fogli_stanno_nella_pagina() {
        for f in formats() {
            let right = f.left + f.cols as f32 * f.label_w + (f.cols - 1) as f32 * f.gap_x;
            let bottom = f.top + f.rows as f32 * f.label_h + (f.rows - 1) as f32 * f.gap_y;
            assert!(right <= f.page_w + 0.01 && bottom <= f.page_h + 0.01, "{}: {} x {}", f.name, right, bottom);
        }
    }

    #[test]
    fn etichette_su_piu_fogli_e_rotolo() {
        let p = LabelContent::Product { title: "Sedia ergonomica da ufficio con braccioli regolabili".into(), code: "SED01".into(), price: Some("€ 149,00".into()), barcode: Some("4006381333931".into()), note: String::new() };
        let a = LabelContent::Address { heading: "Spett.le".into(), lines: vec!["Rossi Arredamenti S.p.A.".into(), "Viale Europa, 10".into(), "00144 Roma (RM)".into()] };
        let s = LabelContent::Shipping { sender: vec!["Bottega Demo S.r.l.".into()], recipient: vec!["Rossi Arredamenti S.p.A.".into(), "Viale Europa, 10".into()], info: vec!["Colli 4 - DDT 1/2026".into()] };
        let sheet = format_by_id("a4-3x8");
        let labels: Vec<LabelContent> = std::iter::repeat_n(p.clone(), 30).chain([a.clone(), s.clone()]).collect();
        let pdf = render_labels(&sheet, &labels, 5, true);
        // 5 saltate + 32 etichette = 37 posizioni = 2 fogli
        assert_eq!(String::from_utf8_lossy(&pdf).matches("/Type /Page ").count(), 2);
        let roll = render_labels(&format_by_id("roll-50x30"), &[p, a, s], 3, false);
        let txt = String::from_utf8_lossy(&roll);
        assert_eq!(txt.matches("/Type /Page ").count(), 3);
        assert!(txt.contains("/MediaBox [0 0 141.73 85.04]"));
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/pdf-samples");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("etichette-a4.pdf"), pdf).unwrap();
        // etichette singole per la verifica della lettura dei codici (vedi verifica.html)
        for (name, code) in [("barcode-ean13.pdf", "4006381333931"), ("barcode-ean8.pdf", "96385074"), ("barcode-c128.pdf", "SED01-XL"), ("barcode-c128-num.pdf", "12345678")] {
            let l = LabelContent::Product { title: "Prova".into(), code: code.into(), price: None, barcode: Some(code.into()), note: String::new() };
            std::fs::write(out.join(name), render_labels(&format_by_id("roll-62x29"), &[l], 0, false)).unwrap();
        }
    }
}
