//! Stampa tabellare generica (registri, elenchi): intestazione ripetuta su ogni pagina,
//! colonne con larghezza relativa, pagine numerate.

use crate::writer::{text_width, Align, Pdf, A4_H, A4_W};

#[derive(Debug, Clone)]
pub struct TableColumn {
    pub title: String,
    /// Larghezza relativa.
    pub width: f32,
    pub right: bool,
}

impl TableColumn {
    pub fn new(title: &str, width: f32, right: bool) -> Self {
        TableColumn { title: title.into(), width, right }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TableRow {
    pub cells: Vec<String>,
    /// Riga in grassetto (totali, intestazioni di gruppo).
    pub bold: bool,
    /// Riga con fondo grigio.
    pub shaded: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TableDoc {
    pub title: String,
    /// Righe sotto il titolo (azienda, periodo).
    pub subtitle: Vec<String>,
    pub columns: Vec<TableColumn>,
    pub rows: Vec<TableRow>,
    /// Testo a piè di pagina (a sinistra); a destra il numero di pagina.
    pub footer: String,
    /// Prefisso della numerazione delle pagine (es. "2026/").
    pub page_prefix: String,
}

const M: f32 = 30.0;
const SIZE: f32 = 7.5;
const ROW_H: f32 = 11.0;

fn fit(s: &str, w: f32, bold: bool) -> String {
    if text_width(s, SIZE, bold) <= w {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        let next = format!("{}{}…", out, c);
        if text_width(&next, SIZE, bold) > w {
            break;
        }
        out.push(c);
    }
    format!("{}…", out.trim_end())
}

pub fn render_table(doc: &TableDoc) -> Vec<u8> {
    let mut pdf = Pdf::new(&doc.title);
    let total_w: f32 = doc.columns.iter().map(|c| c.width).sum::<f32>().max(1.0);
    let avail = A4_W - 2.0 * M;
    let mut xs = Vec::new();
    let mut x = M;
    for c in &doc.columns {
        let w = c.width / total_w * avail;
        xs.push((x, w));
        x += w;
    }
    let mut page = 1;
    let header = |pdf: &mut Pdf, page: usize| -> f32 {
        let mut y = M + 12.0;
        pdf.text(M, y, 13.0, true, &doc.title, Align::Left);
        y += 14.0;
        for s in &doc.subtitle {
            pdf.text(M, y, 8.0, false, s, Align::Left);
            y += 10.0;
        }
        y += 6.0;
        pdf.rect(M, y - 9.0, avail, 13.0, Some(0.90), false);
        for (c, (x, w)) in doc.columns.iter().zip(&xs) {
            let t = fit(&c.title, w - 4.0, true);
            if c.right {
                pdf.text(x + w - 2.0, y, SIZE, true, &t, Align::Right);
            } else {
                pdf.text(x + 2.0, y, SIZE, true, &t, Align::Left);
            }
        }
        pdf.text(A4_W - M, A4_H - 18.0, 7.0, false, &format!("Pagina {}{}", doc.page_prefix, page), Align::Right);
        if !doc.footer.is_empty() {
            pdf.text(M, A4_H - 18.0, 7.0, false, &doc.footer, Align::Left);
        }
        y + ROW_H + 2.0
    };
    let mut y = header(&mut pdf, page);
    for r in &doc.rows {
        if y > A4_H - 40.0 {
            pdf.new_page();
            page += 1;
            y = header(&mut pdf, page);
        }
        if r.shaded {
            pdf.rect(M, y - 8.5, avail, ROW_H, Some(0.95), false);
        }
        for (i, (c, (x, w))) in doc.columns.iter().zip(&xs).enumerate() {
            let v = r.cells.get(i).map(|s| s.as_str()).unwrap_or("");
            let t = fit(v, w - 4.0, r.bold);
            if c.right {
                pdf.text(x + w - 2.0, y, SIZE, r.bold, &t, Align::Right);
            } else {
                pdf.text(x + 2.0, y, SIZE, r.bold, &t, Align::Left);
            }
        }
        if r.bold {
            pdf.line(M, y - 9.0, M + avail, y - 9.0, 0.4);
        }
        y += ROW_H;
    }
    pdf.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabella_su_piu_pagine() {
        let doc = TableDoc {
            title: "Registro IVA vendite".into(),
            subtitle: vec!["Bottega Demo S.r.l. - P.IVA 12345678903".into(), "Marzo 2026".into()],
            columns: vec![TableColumn::new("Data", 1.0, false), TableColumn::new("Cliente", 4.0, false), TableColumn::new("Imponibile", 1.5, true)],
            rows: (0..150).map(|i| TableRow { cells: vec!["01/03/2026".into(), format!("Cliente con un nome molto lungo che va tagliato per stare nella colonna {}", i), "1.234,56".into()], ..Default::default() }).collect(),
            footer: "openFatt".into(),
            page_prefix: "2026/".into(),
        };
        let pdf = render_table(&doc);
        assert!(pdf.starts_with(b"%PDF"));
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("/Count 3") || s.matches("/Type /Page ").count() >= 3);
    }
}
