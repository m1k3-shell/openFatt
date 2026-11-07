//! Modelli di stampa liberi: ogni elemento ha posizione e misura in millimetri.
//!
//! Servono per impaginare a piacere i documenti e per stampare sui **moduli prestampati**
//! (fogli già intestati, bollettari, moduli a ricalco): in quel caso si stampano solo i dati,
//! nelle posizioni dei campi del modulo, con una correzione dell'allineamento della stampante.
//! La scansione del modulo serve solo all'editor come sfondo e, se richiesto, si può stampare
//! su carta bianca.

use serde::{Deserialize, Serialize};

use crate::writer::{wrap, Align, Image, Pdf};
use crate::{PrintDoc, PrintLine};

/// Punti tipografici per millimetro.
pub const PT_MM: f32 = 72.0 / 25.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
pub enum FieldKind {
    /// Testo fisso scritto nel modello
    #[default]
    Text,
    CompanyName,
    /// Dati dell'azienda su più righe
    CompanyBlock,
    /// Cliente o fornitore: nome e indirizzo
    PartyBlock,
    DestinationBlock,
    DocTitle,
    DocNumber,
    DocDate,
    /// Un dato della testata per etichetta (es. "Pagamento", "Colli", "Causale trasp.")
    Info,
    /// Tutte le voci della testata su più righe
    InfoBlock,
    VatSummary,
    Taxable,
    Tax,
    Total,
    NetToPay,
    TotalsBlock,
    DueDates,
    Notes,
    PageNumber,
    Logo,
    /// Riquadro (solo bordo)
    Box,
    /// Linea orizzontale
    Line,
}

impl FieldKind {
    pub const ALL: [FieldKind; 22] = [
        FieldKind::Text,
        FieldKind::CompanyName,
        FieldKind::CompanyBlock,
        FieldKind::PartyBlock,
        FieldKind::DestinationBlock,
        FieldKind::DocTitle,
        FieldKind::DocNumber,
        FieldKind::DocDate,
        FieldKind::Info,
        FieldKind::InfoBlock,
        FieldKind::VatSummary,
        FieldKind::Taxable,
        FieldKind::Tax,
        FieldKind::Total,
        FieldKind::NetToPay,
        FieldKind::TotalsBlock,
        FieldKind::DueDates,
        FieldKind::Notes,
        FieldKind::PageNumber,
        FieldKind::Logo,
        FieldKind::Box,
        FieldKind::Line,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FieldKind::Text => "Testo fisso",
            FieldKind::CompanyName => "Ragione sociale dell'azienda",
            FieldKind::CompanyBlock => "Dati dell'azienda",
            FieldKind::PartyBlock => "Cliente / fornitore",
            FieldKind::DestinationBlock => "Destinazione",
            FieldKind::DocTitle => "Titolo del documento",
            FieldKind::DocNumber => "Numero",
            FieldKind::DocDate => "Data",
            FieldKind::Info => "Dato della testata",
            FieldKind::InfoBlock => "Dati della testata (tutti)",
            FieldKind::VatSummary => "Riepilogo IVA",
            FieldKind::Taxable => "Imponibile",
            FieldKind::Tax => "IVA",
            FieldKind::Total => "Totale documento",
            FieldKind::NetToPay => "Netto a pagare",
            FieldKind::TotalsBlock => "Totali (tutti)",
            FieldKind::DueDates => "Scadenze",
            FieldKind::Notes => "Note",
            FieldKind::PageNumber => "Numero di pagina",
            FieldKind::Logo => "Logo",
            FieldKind::Box => "Riquadro",
            FieldKind::Line => "Linea",
        }
    }

    /// Elementi grafici o testi fissi, già presenti sul modulo prestampato.
    pub fn is_decoration(self) -> bool {
        matches!(self, FieldKind::Text | FieldKind::Logo | FieldKind::Box | FieldKind::Line)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FreeField {
    pub kind: FieldKind,
    /// Posizione e misura in millimetri dall'angolo in alto a sinistra
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub size: f32,
    pub bold: bool,
    /// 0 sinistra, 1 centro, 2 destra
    pub align: u8,
    /// Testo fisso, oppure etichetta del dato della testata (FieldKind::Info)
    pub text: String,
    /// Stampato anche sui moduli prestampati (di norma no per testi fissi e grafica)
    pub on_preprinted: bool,
    /// Solo nell'ultima pagina (totali, scadenze, note)
    pub last_page_only: bool,
}

impl Default for FreeField {
    fn default() -> Self {
        FreeField { kind: FieldKind::Text, x: 20.0, y: 20.0, w: 60.0, h: 8.0, size: 9.0, bold: false, align: 0, text: String::new(), on_preprinted: false, last_page_only: false }
    }
}

impl FreeField {
    pub fn new(kind: FieldKind, x: f32, y: f32, w: f32, h: f32) -> Self {
        let last = matches!(kind, FieldKind::VatSummary | FieldKind::Taxable | FieldKind::Tax | FieldKind::Total | FieldKind::NetToPay | FieldKind::TotalsBlock | FieldKind::DueDates | FieldKind::Notes);
        FreeField { kind, x, y, w, h, on_preprinted: !kind.is_decoration(), last_page_only: last, ..Default::default() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColKey {
    Code,
    #[default]
    Description,
    Qty,
    Unit,
    Price,
    Discount,
    Amount,
    Vat,
}

impl ColKey {
    pub const ALL: [ColKey; 8] = [ColKey::Code, ColKey::Description, ColKey::Qty, ColKey::Unit, ColKey::Price, ColKey::Discount, ColKey::Amount, ColKey::Vat];
    pub fn label(self) -> &'static str {
        match self {
            ColKey::Code => "Codice",
            ColKey::Description => "Descrizione",
            ColKey::Qty => "Q.tà",
            ColKey::Unit => "U.M.",
            ColKey::Price => "Prezzo",
            ColKey::Discount => "Sconto %",
            ColKey::Amount => "Importo",
            ColKey::Vat => "IVA",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FreeColumn {
    pub key: ColKey,
    /// Posizione dalla sinistra dell'area righe e larghezza, in millimetri
    pub x: f32,
    pub w: f32,
    pub align: u8,
}

/// Area in cui scorrono le righe del documento (si continua nelle pagine seguenti).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TableArea {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub row_h: f32,
    pub size: f32,
    /// Intestazione delle colonne (di norma no sui moduli prestampati)
    pub header: bool,
    pub columns: Vec<FreeColumn>,
}

impl Default for TableArea {
    fn default() -> Self {
        TableArea {
            x: 12.0,
            y: 100.0,
            w: 186.0,
            h: 120.0,
            row_h: 4.6,
            size: 8.0,
            header: true,
            columns: vec![
                FreeColumn { key: ColKey::Code, x: 0.0, w: 22.0, align: 0 },
                FreeColumn { key: ColKey::Description, x: 23.0, w: 82.0, align: 0 },
                FreeColumn { key: ColKey::Qty, x: 106.0, w: 14.0, align: 2 },
                FreeColumn { key: ColKey::Unit, x: 121.0, w: 9.0, align: 0 },
                FreeColumn { key: ColKey::Price, x: 131.0, w: 18.0, align: 2 },
                FreeColumn { key: ColKey::Discount, x: 150.0, w: 12.0, align: 2 },
                FreeColumn { key: ColKey::Amount, x: 163.0, w: 15.0, align: 2 },
                FreeColumn { key: ColKey::Vat, x: 179.0, w: 7.0, align: 2 },
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FreeLayout {
    pub name: String,
    /// Codici dei tipi di documento che usano questo modello (FAT, DDT...)
    pub kinds: Vec<String>,
    pub page_w: f32,
    pub page_h: f32,
    /// Modulo prestampato: si stampano solo i dati
    pub preprinted: bool,
    /// Correzione dell'allineamento della stampante (mm, positivo = verso destra / in basso)
    pub offset_x: f32,
    pub offset_y: f32,
    /// Scansione del modulo (PNG o JPEG in base64), sfondo dell'editor
    pub background: String,
    /// Stampa anche lo sfondo (per provare su carta bianca)
    pub print_background: bool,
    pub fields: Vec<FreeField>,
    pub table: TableArea,
}

impl Default for FreeLayout {
    fn default() -> Self {
        FreeLayout::standard_a4("Nuovo modello")
    }
}

impl FreeLayout {
    /// Modello di partenza A4: intestazione, destinatario, dati, righe e totali.
    pub fn standard_a4(name: &str) -> FreeLayout {
        let mut f = vec![
            FreeField { size: 7.0, ..FreeField::new(FieldKind::Logo, 12.0, 10.0, 40.0, 18.0) },
            FreeField { size: 12.0, bold: true, ..FreeField::new(FieldKind::CompanyName, 12.0, 30.0, 90.0, 6.0) },
            FreeField { size: 8.0, ..FreeField::new(FieldKind::CompanyBlock, 12.0, 36.0, 90.0, 22.0) },
            FreeField { size: 15.0, bold: true, align: 2, ..FreeField::new(FieldKind::DocTitle, 120.0, 12.0, 78.0, 8.0) },
            FreeField { size: 10.0, align: 2, ..FreeField::new(FieldKind::DocNumber, 120.0, 21.0, 78.0, 6.0) },
            FreeField { size: 10.0, align: 2, ..FreeField::new(FieldKind::DocDate, 120.0, 27.0, 78.0, 6.0) },
            FreeField { size: 9.0, ..FreeField::new(FieldKind::PartyBlock, 115.0, 45.0, 83.0, 28.0) },
            FreeField { size: 8.0, ..FreeField::new(FieldKind::InfoBlock, 12.0, 62.0, 95.0, 32.0) },
            FreeField { size: 8.0, ..FreeField::new(FieldKind::DestinationBlock, 115.0, 76.0, 83.0, 18.0) },
            FreeField { size: 8.0, ..FreeField::new(FieldKind::VatSummary, 12.0, 228.0, 110.0, 20.0) },
            FreeField { size: 9.0, ..FreeField::new(FieldKind::TotalsBlock, 128.0, 228.0, 70.0, 26.0) },
            FreeField { size: 8.0, ..FreeField::new(FieldKind::DueDates, 12.0, 252.0, 110.0, 16.0) },
            FreeField { size: 7.5, ..FreeField::new(FieldKind::Notes, 12.0, 268.0, 186.0, 16.0) },
            FreeField { size: 7.0, align: 2, ..FreeField::new(FieldKind::PageNumber, 150.0, 288.0, 48.0, 5.0) },
        ];
        for x in f.iter_mut() {
            x.last_page_only = matches!(x.kind, FieldKind::VatSummary | FieldKind::TotalsBlock | FieldKind::DueDates | FieldKind::Notes);
        }
        f.push(FreeField { kind: FieldKind::Box, x: 113.0, y: 43.0, w: 87.0, h: 31.0, on_preprinted: false, ..Default::default() });
        FreeLayout { name: name.into(), kinds: vec![], page_w: 210.0, page_h: 297.0, preprinted: false, offset_x: 0.0, offset_y: 0.0, background: String::new(), print_background: false, fields: f, table: TableArea::default() }
    }

    pub fn applies_to(&self, kind_code: &str) -> bool {
        self.kinds.iter().any(|k| k == kind_code)
    }

    /// Righe del documento che entrano in una pagina.
    pub fn rows_per_page(&self) -> usize {
        let header = if self.table.header && !self.preprinted { 1.0 } else { 0.0 };
        (((self.table.h / self.table.row_h.max(2.0)) - header).floor() as usize).max(1)
    }
}

fn mm(v: f32) -> f32 {
    v * PT_MM
}

fn align_of(a: u8) -> Align {
    match a {
        1 => Align::Center,
        2 => Align::Right,
        _ => Align::Left,
    }
}

/// Scrive righe di testo nel riquadro, tagliando quelle che non ci stanno.
fn block(pdf: &mut Pdf, l: &FreeLayout, f: &FreeField, lines: &[String]) {
    let size = f.size.max(4.0);
    let lh = size * 1.25;
    let x0 = mm(f.x + l.offset_x);
    let y0 = mm(f.y + l.offset_y);
    let w = mm(f.w);
    let max_lines = ((mm(f.h) / lh).floor() as usize).max(1);
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        for part in wrap(line, w, size, f.bold) {
            out.push(part);
        }
    }
    let x = match f.align {
        1 => x0 + w / 2.0,
        2 => x0 + w,
        _ => x0,
    };
    for (i, t) in out.iter().take(max_lines).enumerate() {
        pdf.text(x, y0 + size + i as f32 * lh, size, f.bold, t, align_of(f.align));
    }
}

/// Coppie etichetta/valore incolonnate (totali, riepilogo, scadenze).
fn pairs(pdf: &mut Pdf, l: &FreeLayout, f: &FreeField, rows: &[(String, String, bool)]) {
    let size = f.size.max(4.0);
    let lh = size * 1.35;
    let x0 = mm(f.x + l.offset_x);
    let y0 = mm(f.y + l.offset_y);
    let w = mm(f.w);
    let max_lines = ((mm(f.h) / lh).floor() as usize).max(1);
    for (i, (a, b, bold)) in rows.iter().take(max_lines).enumerate() {
        let y = y0 + size + i as f32 * lh;
        pdf.text(x0, y, size, *bold || f.bold, a, Align::Left);
        pdf.text(x0 + w, y, size, *bold || f.bold, b, Align::Right);
    }
}

fn cell(line: &PrintLine, key: ColKey) -> &str {
    match key {
        ColKey::Code => &line.code,
        ColKey::Description => &line.description,
        ColKey::Qty => &line.qty,
        ColKey::Unit => &line.unit,
        ColKey::Price => &line.price,
        ColKey::Discount => &line.discount,
        ColKey::Amount => &line.total,
        ColKey::Vat => &line.vat,
    }
}

/// Le righe del documento spezzate in righe di stampa (le descrizioni lunghe vanno a capo).
fn table_rows(l: &FreeLayout, doc: &PrintDoc) -> Vec<Vec<(ColKey, String)>> {
    let size = l.table.size.max(4.0);
    let desc_w = l.table.columns.iter().find(|c| c.key == ColKey::Description).map(|c| mm(c.w)).unwrap_or(mm(80.0));
    let mut out = Vec::new();
    for line in &doc.lines {
        let parts = wrap(&line.description, desc_w, size, line.is_text);
        for (i, p) in parts.iter().enumerate() {
            let mut row = Vec::new();
            for c in &l.table.columns {
                let v = if c.key == ColKey::Description {
                    p.clone()
                } else if i == 0 && (doc.show_prices || !matches!(c.key, ColKey::Price | ColKey::Discount | ColKey::Amount | ColKey::Vat)) {
                    cell(line, c.key).to_string()
                } else {
                    String::new()
                };
                row.push((c.key, v));
            }
            out.push(row);
        }
    }
    out
}

/// Disegna un documento con il modello libero (tutte le pagine).
pub fn draw(pdf: &mut Pdf, l: &FreeLayout, doc: &PrintDoc, logo: Option<usize>, background: Option<usize>, first: bool) {
    let rows = table_rows(l, doc);
    let per_page = l.rows_per_page();
    let pages = rows.len().div_ceil(per_page).max(1);
    for page in 0..pages {
        if !(first && page == 0) {
            pdf.new_page();
        }
        let last = page + 1 == pages;
        if let (Some(bg), true) = (background, l.print_background) {
            pdf.image(bg, mm(l.offset_x), mm(l.offset_y), mm(l.page_w), mm(l.page_h));
        }
        for f in &l.fields {
            if l.preprinted && !f.on_preprinted {
                continue;
            }
            if f.last_page_only && !last {
                continue;
            }
            draw_field(pdf, l, f, doc, logo, page + 1, pages);
        }
        // righe
        let t = &l.table;
        let x0 = mm(t.x + l.offset_x);
        let mut y = mm(t.y + l.offset_y);
        let size = t.size.max(4.0);
        if t.header && !l.preprinted {
            for c in &t.columns {
                let x = x0 + mm(c.x) + if c.align == 2 { mm(c.w) } else if c.align == 1 { mm(c.w) / 2.0 } else { 0.0 };
                pdf.text(x, y + size, size, true, c.key.label(), align_of(c.align));
            }
            pdf.line(x0, y + mm(t.row_h) - 1.0, x0 + mm(t.w), y + mm(t.row_h) - 1.0, 0.4);
            y += mm(t.row_h);
        }
        for row in rows.iter().skip(page * per_page).take(per_page) {
            for (c, (_, v)) in t.columns.iter().zip(row.iter()) {
                if v.is_empty() {
                    continue;
                }
                let x = x0 + mm(c.x) + if c.align == 2 { mm(c.w) } else if c.align == 1 { mm(c.w) / 2.0 } else { 0.0 };
                let text = if c.key == ColKey::Description { v.clone() } else { crate::writer::wrap(v, mm(c.w) + 2.0, size, false).first().cloned().unwrap_or_default() };
                pdf.text(x, y + size, size, false, &text, align_of(c.align));
            }
            y += mm(t.row_h);
        }
        if !last {
            pdf.text(x0 + mm(t.w), mm(t.y + t.h + l.offset_y) + size, size, false, "segue...", Align::Right);
        }
    }
}

fn draw_field(pdf: &mut Pdf, l: &FreeLayout, f: &FreeField, doc: &PrintDoc, logo: Option<usize>, page: usize, pages: usize) {
    let one = |pdf: &mut Pdf, s: String| block(pdf, l, f, &[s]);
    match f.kind {
        FieldKind::Text => block(pdf, l, f, &f.text.lines().map(|s| s.to_string()).collect::<Vec<_>>()),
        FieldKind::CompanyName => one(pdf, doc.issuer.first().cloned().unwrap_or_default()),
        FieldKind::CompanyBlock => block(pdf, l, f, &doc.issuer.iter().skip(1).cloned().collect::<Vec<_>>()),
        FieldKind::PartyBlock => block(pdf, l, f, &doc.recipient),
        FieldKind::DestinationBlock => {
            if !doc.destination.is_empty() {
                let mut v = vec!["Destinazione:".to_string()];
                v.extend(doc.destination.iter().cloned());
                block(pdf, l, f, &v);
            }
        }
        FieldKind::DocTitle => one(pdf, doc.title.clone()),
        FieldKind::DocNumber => one(pdf, if f.text.is_empty() { format!("N. {}", doc.number) } else { format!("{}{}", f.text, doc.number) }),
        FieldKind::DocDate => one(pdf, if f.text.is_empty() { format!("del {}", doc.date) } else { format!("{}{}", f.text, doc.date) }),
        FieldKind::Info => {
            if let Some((_, v)) = doc.info.iter().find(|(k, _)| k.eq_ignore_ascii_case(f.text.trim())) {
                one(pdf, v.clone());
            }
        }
        FieldKind::InfoBlock => block(pdf, l, f, &doc.info.iter().map(|(k, v)| format!("{}: {}", k, v)).collect::<Vec<_>>()),
        FieldKind::VatSummary => {
            if doc.show_prices {
                let mut rows = vec![("Aliquota  Imponibile".to_string(), "Imposta".to_string(), true)];
                rows.extend(doc.vat_rows.iter().map(|(a, t, i, _)| (format!("{}  {}", a, t), i.clone(), false)));
                pairs(pdf, l, f, &rows);
            }
        }
        FieldKind::Taxable | FieldKind::Tax | FieldKind::Total | FieldKind::NetToPay => {
            if doc.show_prices {
                let key = match f.kind {
                    FieldKind::Taxable => "Imponibile",
                    FieldKind::Tax => "IVA",
                    FieldKind::Total => "Totale documento",
                    _ => "Netto a pagare",
                };
                let v = doc.totals.iter().find(|(k, _, _)| k == key).or_else(|| if f.kind == FieldKind::NetToPay { doc.totals.iter().find(|(k, _, _)| k == "Totale documento") } else { None });
                if let Some((_, v, _)) = v {
                    one(pdf, v.clone());
                }
            }
        }
        FieldKind::TotalsBlock => {
            if doc.show_prices {
                pairs(pdf, l, f, &doc.totals.iter().map(|(a, b, c)| (a.clone(), b.clone(), *c)).collect::<Vec<_>>());
            }
        }
        FieldKind::DueDates => {
            if !doc.payments.is_empty() {
                let mut rows = vec![("Scadenze".to_string(), String::new(), true)];
                rows.extend(doc.payments.iter().map(|(d, m, a)| (format!("{}  {}", d, m), a.clone(), false)));
                pairs(pdf, l, f, &rows);
            }
        }
        FieldKind::Notes => block(pdf, l, f, &doc.notes),
        FieldKind::PageNumber => one(pdf, format!("{}{}/{}", if f.text.is_empty() { "Pagina " } else { &f.text }, page, pages)),
        FieldKind::Logo => {
            if let Some(i) = logo {
                pdf.image(i, mm(f.x + l.offset_x), mm(f.y + l.offset_y), mm(f.w), mm(f.h));
            }
        }
        FieldKind::Box => pdf.rect(mm(f.x + l.offset_x), mm(f.y + l.offset_y), mm(f.w), mm(f.h), None, true),
        FieldKind::Line => pdf.line(mm(f.x + l.offset_x), mm(f.y + l.offset_y), mm(f.x + f.w + l.offset_x), mm(f.y + l.offset_y), 0.6),
    }
}

/// PDF di uno o più documenti con il modello libero.
pub fn render(l: &FreeLayout, docs: &[&PrintDoc], logo: Option<&Image>) -> Vec<u8> {
    let mut pdf = Pdf::with_size(docs.first().map(|d| d.title.as_str()).unwrap_or("Documento"), mm(l.page_w), mm(l.page_h));
    let logo = logo.map(|i| pdf.add_image(i));
    let bg = if l.print_background { background_image(l).map(|i| pdf.add_image(&i)) } else { None };
    for (i, d) in docs.iter().enumerate() {
        draw(&mut pdf, l, d, logo, bg, i == 0);
    }
    pdf.finish()
}

/// Immagine di sfondo del modello (scansione del modulo).
pub fn background_image(l: &FreeLayout) -> Option<Image> {
    use base64::Engine;
    if l.background.is_empty() {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(l.background.as_bytes()).ok()?;
    Image::from_bytes(&bytes).ok()
}

/// Documento di esempio per l'anteprima dei modelli quando l'archivio non ne ha.
pub fn sample_doc() -> PrintDoc {
sample(6)
}

fn sample(lines: usize) -> PrintDoc {
    PrintDoc {
        title: "Fattura".into(),
        number: "12".into(),
        date: "07/11/2026".into(),
        issuer: vec!["Bottega Demo S.r.l.".into(), "Via delle Prove 1".into(), "38122 Trento".into()],
        recipient: vec!["Rossi Arredamenti S.p.A.".into(), "Via Roma 10".into()],
        info: vec![("Pagamento".into(), "Bonifico 30 gg".into()), ("Colli".into(), "3".into())],
        lines: (0..lines).map(|i| PrintLine { code: format!("A{}", i), description: format!("Articolo numero {} con una descrizione abbastanza lunga da andare a capo nella colonna", i), qty: "1".into(), unit: "pz".into(), price: "10,00".into(), discount: String::new(), total: "10,00".into(), vat: "22%".into(), is_text: false }).collect(),
        vat_rows: vec![("22%".into(), "100,00".into(), "22,00".into(), String::new())],
        totals: vec![("Imponibile".into(), "100,00".into(), false), ("IVA".into(), "22,00".into(), false), ("Totale documento".into(), "EUR 122,00".into(), true)],
        payments: vec![("07/12/2026".into(), "Bonifico".into(), "122,00".into())],
        notes: vec!["Grazie".into()],
        show_prices: true,
        ..Default::default()
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_paginates_and_respects_preprinted() {
        let mut l = FreeLayout::standard_a4("Prova");
        let few = render(&l, &[&sample(3)], None);
        assert!(few.starts_with(b"%PDF"));
        assert!(String::from_utf8_lossy(&few).contains("/Count 1 >>"));
        // molte righe: più pagine, "segue..." e i totali solo in fondo
        let many = render(&l, &[&sample(60)], None);
        let s = String::from_utf8_lossy(&many);
        assert!(!s.contains("/Count 1 >>"), "attese più pagine");
        assert!(s.contains("segue..."));
        // modulo prestampato: niente testi fissi, riquadri e intestazione delle colonne
        l.preprinted = true;
        l.fields.push(FreeField { kind: FieldKind::Text, text: "SOLO SUL MODULO".into(), ..Default::default() });
        l.offset_x = 2.0;
        let pre = String::from_utf8_lossy(&render(&l, &[&sample(3)], None)).to_string();
        assert!(!pre.contains("SOLO SUL MODULO"));
        assert!(!pre.contains("(Descrizione)"));
        assert!(pre.contains("Rossi Arredamenti"));
        // il modello si salva e si rilegge
        let js = serde_json::to_string(&l).unwrap();
        let back: FreeLayout = serde_json::from_str(&js).unwrap();
        assert_eq!(back, l);
        assert!(FreeLayout::standard_a4("x").rows_per_page() > 20);
    }
}
