//! Stampa su rotolo (stampanti termiche da 80 o 58 mm): lo stesso documento impaginato in
//! una colonna stretta, con la pagina alta quanto serve. Si stampa dal PDF con il driver
//! della stampante, senza comandi proprietari.

use crate::writer::{text_width, wrap, Align, Pdf};
use crate::PrintDoc;

const MM: f32 = 72.0 / 25.4;

/// Altezza minima di una pagina (rotolo), in punti.
const MIN_H: f32 = 200.0;

/// Una riga da disegnare: testo a sinistra, testo a destra, corpo, grassetto, separatore.
enum Row {
    Text { left: String, right: String, size: f32, bold: bool, center: bool },
    Rule,
    Gap(f32),
}

fn rows(doc: &PrintDoc, width: f32) -> Vec<Row> {
    let mut out = Vec::new();
    let t = |left: &str, size: f32, bold: bool, center: bool| Row::Text { left: left.to_string(), right: String::new(), size, bold, center };
    for (i, s) in doc.issuer.iter().enumerate() {
        for w in wrap(s, width, if i == 0 { 10.0 } else { 8.0 }, i == 0) {
            out.push(t(&w, if i == 0 { 10.0 } else { 8.0 }, i == 0, true));
        }
    }
    out.push(Row::Rule);
    out.push(t(&doc.title.to_uppercase(), 10.0, true, true));
    out.push(t(&format!("N. {} del {}", doc.number, doc.date), 8.5, false, true));
    if let Some(w) = &doc.watermark {
        out.push(t(w, 8.0, true, true));
    }
    if !doc.recipient.is_empty() {
        out.push(Row::Gap(4.0));
        out.push(t(&doc.recipient_title, 7.0, true, false));
        for s in &doc.recipient {
            for w in wrap(s, width, 8.0, false) {
                out.push(t(&w, 8.0, false, false));
            }
        }
    }
    for (k, v) in &doc.info {
        for w in wrap(&format!("{}: {}", k, v), width, 7.5, false) {
            out.push(t(&w, 7.5, false, false));
        }
    }
    out.push(Row::Rule);
    for l in &doc.lines {
        if l.is_text {
            for w in wrap(&l.description, width, 8.0, false) {
                out.push(t(&w, 8.0, false, false));
            }
            continue;
        }
        let desc = if l.code.trim().is_empty() { l.description.clone() } else { format!("{} {}", l.code, l.description) };
        for w in wrap(&desc, width, 8.5, false) {
            out.push(t(&w, 8.5, false, false));
        }
        if doc.show_prices {
            let qty = format!("  {} {} x {}{}", l.qty, l.unit, l.price, if l.discount.is_empty() { String::new() } else { format!(" -{}", l.discount) });
            out.push(Row::Text { left: qty, right: l.total.clone(), size: 8.0, bold: false, center: false });
        } else {
            out.push(Row::Text { left: format!("  {} {}", l.qty, l.unit), right: String::new(), size: 8.0, bold: false, center: false });
        }
    }
    if doc.show_prices {
        out.push(Row::Rule);
        for (k, v, strong) in &doc.totals {
            out.push(Row::Text { left: k.clone(), right: v.clone(), size: if *strong { 10.0 } else { 8.5 }, bold: *strong, center: false });
        }
        if !doc.vat_rows.is_empty() {
            out.push(Row::Gap(3.0));
            for (rate, taxable, tax, _) in &doc.vat_rows {
                out.push(Row::Text { left: format!("IVA {} su {}", rate, taxable), right: tax.clone(), size: 7.5, bold: false, center: false });
            }
        }
        for (date, amount, how) in &doc.payments {
            out.push(Row::Text { left: format!("{} {}", date, how), right: amount.clone(), size: 7.5, bold: false, center: false });
        }
    }
    if !doc.notes.is_empty() {
        out.push(Row::Rule);
        for n in &doc.notes {
            for part in n.lines() {
                for w in wrap(part, width, 7.5, false) {
                    out.push(t(&w, 7.5, false, false));
                }
            }
        }
    }
    if !doc.footer.trim().is_empty() {
        out.push(Row::Gap(4.0));
        for w in wrap(doc.footer.trim(), width, 7.0, false) {
            out.push(t(&w, 7.0, false, true));
        }
    }
    out
}

fn height_of(r: &Row) -> f32 {
    match r {
        Row::Text { size, .. } => size * 1.35,
        Row::Rule => 8.0,
        Row::Gap(g) => *g,
    }
}

/// PDF su rotolo largo `paper_mm` (80 o 58), margini di 4 mm.
pub fn render_roll(docs: &[PrintDoc], paper_mm: f32) -> Vec<u8> {
    let w = paper_mm * MM;
    let margin = 4.0 * MM;
    let width = w - 2.0 * margin;
    let all: Vec<Vec<Row>> = docs.iter().map(|d| rows(d, width)).collect();
    // una pagina per documento, alta quanto il documento più lungo
    let h = all.iter().map(|r| r.iter().map(height_of).sum::<f32>() + 2.0 * margin + 10.0).fold(MIN_H, f32::max);
    let title = docs.first().map(|d| format!("{} {}", d.title, d.number)).unwrap_or_default();
    let mut pdf = Pdf::with_size(&title, w, h);
    for (i, doc_rows) in all.iter().enumerate() {
        if i > 0 {
            pdf.new_page();
        }
        let mut y = margin + 8.0;
        for r in doc_rows {
            match r {
                Row::Text { left, right, size, bold, center } => {
                    if *center {
                        pdf.text(w / 2.0, y, *size, *bold, left, Align::Center);
                    } else {
                        let room = width - if right.is_empty() { 0.0 } else { text_width(right, *size, *bold) + 6.0 };
                        let mut l = left.clone();
                        while !l.is_empty() && text_width(&l, *size, *bold) > room {
                            l.pop();
                        }
                        pdf.text(margin, y, *size, *bold, &l, Align::Left);
                        if !right.is_empty() {
                            pdf.text(w - margin, y, *size, *bold, right, Align::Right);
                        }
                    }
                    y += size * 1.35;
                }
                Row::Rule => {
                    pdf.line(margin, y - 4.0, w - margin, y - 4.0, 0.4);
                    y += 8.0;
                }
                Row::Gap(g) => y += g,
            }
        }
    }
    pdf.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PrintLine;

    #[test]
    fn rotolo_80mm_largo_giusto_e_con_tutte_le_righe() {
        let mut d = PrintDoc { title: "DDT".into(), number: "12".into(), date: "28/11/2026".into(), show_prices: true, ..Default::default() };
        d.issuer = vec!["Azienda Srl".into(), "Via Roma 1, Trento".into()];
        for i in 0..40 {
            d.lines.push(PrintLine { code: format!("A{}", i), description: "Articolo con una descrizione piuttosto lunga da andare a capo".into(), qty: "1".into(), unit: "pz".into(), price: "10,00".into(), total: "10,00".into(), ..Default::default() });
        }
        d.totals.push(("Totale".into(), "400,00".into(), true));
        let pdf = render_roll(&[d.clone(), d], 80.0);
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.starts_with("%PDF-1.4"));
        // 80 mm = 226,77 punti
        assert!(s.contains("226.77"), "larghezza della pagina");
        assert!(s.contains("/Count 2"));
        assert!(s.contains("(Totale)"));
        assert!(s.matches("(A39 ").count() >= 2 || s.contains("A39"));
    }
}
