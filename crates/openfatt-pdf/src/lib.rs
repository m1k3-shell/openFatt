//! Stampa PDF dei documenti (fatture, DDT, preventivi...) e delle fatture elettroniche ricevute.
//! Entrambe le stampe passano da `PrintDoc`, una rappresentazione neutra già calcolata.

pub mod barcode;
pub mod freeform;
pub mod labels;
pub mod table;
pub mod writer;

use openfatt_core::calc::{describe_installments, Totals};
use openfatt_core::codes;
use openfatt_core::model::{Company, DocKind, Document, LineKind};
use openfatt_core::money::{fmt_eur, fmt_it_trim};
use rust_decimal::Decimal;

use writer::{wrap, Align, Pdf, A4_H, A4_W};
pub use writer::Image;

/// Impaginazione del documento.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Template {
    /// Riquadri grigi, come i moduli tradizionali.
    #[default]
    Classic,
    /// Colore aziendale su titolo, intestazione delle righe e totale.
    Modern,
    /// Solo linee sottili, nessun fondo.
    Essential,
}

/// Aspetto della stampa: modello, colore, logo e colonne visibili.
#[derive(Debug, Clone, PartialEq)]
pub struct PrintStyle {
    pub template: Template,
    pub accent: (u8, u8, u8),
    pub logo: Option<Image>,
    pub show_codes: bool,
    pub show_discount: bool,
    pub show_vat: bool,
    pub page_numbers: bool,
}

impl Default for PrintStyle {
    fn default() -> Self {
        PrintStyle { template: Template::Classic, accent: (0x1d, 0x5f, 0xbf), logo: None, show_codes: true, show_discount: true, show_vat: true, page_numbers: true }
    }
}

impl PrintStyle {
    fn accent_f(&self) -> (f32, f32, f32) {
        (self.accent.0 as f32 / 255.0, self.accent.1 as f32 / 255.0, self.accent.2 as f32 / 255.0)
    }
    /// Colore aziendale schiarito (per i fondi).
    fn accent_light(&self) -> (f32, f32, f32) {
        let (r, g, b) = self.accent_f();
        (0.86 + r * 0.14, 0.86 + g * 0.14, 0.86 + b * 0.14)
    }
}

#[derive(Debug, Clone, Default)]
pub struct PrintLine {
    pub code: String,
    pub description: String,
    pub qty: String,
    pub unit: String,
    pub price: String,
    pub discount: String,
    pub total: String,
    pub vat: String,
    pub is_text: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PrintDoc {
    pub title: String,
    pub number: String,
    pub date: String,
    pub issuer: Vec<String>,
    pub recipient_title: String,
    pub recipient: Vec<String>,
    pub destination: Vec<String>,
    pub info: Vec<(String, String)>,
    pub lines: Vec<PrintLine>,
    /// aliquota/natura, imponibile, imposta, descrizione
    pub vat_rows: Vec<(String, String, String, String)>,
    pub totals: Vec<(String, String, bool)>,
    pub payments: Vec<(String, String, String)>,
    pub notes: Vec<String>,
    pub footer: String,
    pub show_prices: bool,
    pub watermark: Option<String>,
    pub style: PrintStyle,
}

const M: f32 = 36.0; // margine

struct Layout {
    pdf: Pdf,
    y: f32,
    page: usize,
    style: PrintStyle,
    logo: Option<usize>,
}

const COLS: [(f32, Align); 8] = [
    (M, Align::Left),          // codice
    (M + 62.0, Align::Left),   // descrizione
    (M + 330.0, Align::Right), // qta
    (M + 336.0, Align::Left),  // um
    (M + 412.0, Align::Right), // prezzo
    (M + 448.0, Align::Right), // sconto
    (M + 500.0, Align::Right), // importo
    (A4_W - M, Align::Right),  // iva
];

impl Layout {
    fn hidden(&self, i: usize, show_prices: bool) -> bool {
        (!show_prices && (4..=7).contains(&i)) || (i == 0 && !self.style.show_codes) || (i == 5 && !self.style.show_discount) || (i == 7 && !self.style.show_vat)
    }

    fn header_row(&mut self, show_prices: bool) {
        let y = self.y;
        let white = (1.0, 1.0, 1.0);
        match self.style.template {
            Template::Classic => self.pdf.rect(M, y - 10.0, A4_W - 2.0 * M, 14.0, Some(0.90), false),
            Template::Modern => self.pdf.rect_rgb(M, y - 10.0, A4_W - 2.0 * M, 14.0, self.style.accent_f()),
            Template::Essential => self.pdf.line(M, y + 4.0, A4_W - M, y + 4.0, 0.6),
        }
        let labels = ["Codice", "Descrizione", "Q.tà", "U.M.", "Prezzo", "Sc.%", "Importo", "IVA"];
        for (i, l) in labels.iter().enumerate() {
            if self.hidden(i, show_prices) {
                continue;
            }
            let (x, a) = if i == 1 && !self.style.show_codes { (M + 4.0, Align::Left) } else { COLS[i] };
            if self.style.template == Template::Modern {
                self.pdf.text_rgb(x, y, 7.5, true, l, a, white);
            } else {
                self.pdf.text(x, y, 7.5, true, l, a);
            }
        }
        self.y += 14.0;
    }

    /// Piè di pagina e numero della pagina.
    fn page_footer(&mut self, doc: &PrintDoc) {
        if !doc.footer.is_empty() {
            self.pdf.text(A4_W / 2.0, A4_H - 28.0, 6.5, false, &doc.footer, Align::Center);
        }
        if self.style.page_numbers {
            self.pdf.text(A4_W - M, A4_H - 18.0, 6.5, false, &format!("{} {} - pagina {}", doc.title, doc.number, self.page), Align::Right);
        }
    }

    fn ensure(&mut self, h: f32, doc: &PrintDoc) {
        if self.y + h > A4_H - 70.0 {
            self.pdf.text(A4_W - M, A4_H - 40.0, 7.0, false, "segue", Align::Right);
            self.page_footer(doc);
            self.pdf.new_page();
            self.page += 1;
            self.y = M + 12.0;
            self.pdf.text(M, self.y, 9.0, true, &format!("{} n. {} del {} (segue)", doc.title, doc.number, doc.date), Align::Left);
            self.y += 18.0;
            self.header_row(doc.show_prices);
        }
    }
}

/// Genera il PDF di un `PrintDoc`.
pub fn render(doc: &PrintDoc) -> Vec<u8> {
    render_many(std::slice::from_ref(doc))
}

/// Più documenti in un solo PDF, ciascuno a partire da una nuova pagina.
pub fn render_many(docs: &[PrintDoc]) -> Vec<u8> {
    let title = match docs {
        [d] => format!("{} {}", d.title, d.number),
        _ => format!("{} documenti", docs.len()),
    };
    let mut l = Layout { pdf: Pdf::new(&title), y: M + 10.0, page: 1, style: PrintStyle::default(), logo: None };
    for (i, d) in docs.iter().enumerate() {
        if i > 0 {
            l.pdf.new_page();
        }
        l.y = M + 10.0;
        l.page = 1;
        l.style = d.style.clone();
        l.logo = d.style.logo.as_ref().map(|img| l.pdf.add_image(img));
        draw_doc(&mut l, d);
    }
    l.pdf.finish()
}

fn draw_doc(l: &mut Layout, doc: &PrintDoc) {
    // logo sopra i dati dell'emittente
    if let (Some(idx), Some(img)) = (l.logo, doc.style.logo.as_ref()) {
        let ratio = img.width as f32 / img.height.max(1) as f32;
        let (mw, mh) = (170.0_f32, 52.0_f32);
        let (w, h) = if mw / ratio <= mh { (mw, mw / ratio) } else { (mh * ratio, mh) };
        l.pdf.image(idx, M, M - 6.0, w, h);
        l.y = M - 6.0 + h + 14.0;
    }

    // intestazione emittente
    for (i, s) in doc.issuer.iter().enumerate() {
        let (size, bold) = if i == 0 { (13.0, true) } else { (8.0, false) };
        l.pdf.text(M, l.y, size, bold, s, Align::Left);
        l.y += if i == 0 { 15.0 } else { 10.0 };
    }
    let header_bottom = l.y;

    // titolo documento a destra
    let tx = A4_W - M;
    match l.style.template {
        Template::Modern => l.pdf.text_rgb(tx, M + 10.0, 17.0, true, &doc.title.to_uppercase(), Align::Right, l.style.accent_f()),
        _ => l.pdf.text(tx, M + 10.0, 15.0, true, &doc.title.to_uppercase(), Align::Right),
    }
    l.pdf.text(tx, M + 28.0, 10.0, false, &format!("N. {}   del {}", doc.number, doc.date), Align::Right);
    if let Some(w) = &doc.watermark {
        l.pdf.text(tx, M + 42.0, 9.0, true, w, Align::Right);
    }

    // riquadro destinatario
    let box_x = A4_W / 2.0 + 10.0;
    let box_w = A4_W - M - box_x;
    let mut by = header_bottom.max(M + 58.0) + 6.0;
    let rec_h = 16.0 + 10.0 * doc.recipient.len() as f32;
    l.pdf.rect(box_x, by, box_w, rec_h, None, true);
    l.pdf.text(box_x + 6.0, by + 11.0, 7.0, true, &doc.recipient_title, Align::Left);
    let mut ry = by + 22.0;
    for (i, s) in doc.recipient.iter().enumerate() {
        l.pdf.text(box_x + 6.0, ry, if i == 0 { 9.5 } else { 8.5 }, i == 0, s, Align::Left);
        ry += 10.0;
    }
    by += rec_h + 6.0;
    if !doc.destination.is_empty() {
        let h = 14.0 + 10.0 * doc.destination.len() as f32;
        l.pdf.rect(box_x, by, box_w, h, None, true);
        l.pdf.text(box_x + 6.0, by + 10.0, 7.0, true, "DESTINAZIONE", Align::Left);
        let mut dy = by + 20.0;
        for s in &doc.destination {
            l.pdf.text(box_x + 6.0, dy, 8.5, false, s, Align::Left);
            dy += 10.0;
        }
        by += h + 6.0;
    }
    // informazioni a sinistra (pagamento, riferimenti...)
    let mut iy = header_bottom.max(M + 58.0) + 14.0;
    for (k, v) in &doc.info {
        l.pdf.text(M, iy, 7.0, true, k, Align::Left);
        for (j, part) in wrap(v, box_x - M - 80.0, 8.0, false).iter().enumerate() {
            l.pdf.text(M + 78.0, iy + j as f32 * 9.5, 8.0, false, part, Align::Left);
            if j > 0 {
                iy += 9.5;
            }
        }
        iy += 11.0;
    }
    l.y = by.max(iy) + 10.0;

    // righe
    l.header_row(doc.show_prices);
    let codes = l.style.show_codes;
    let desc_x = if codes { COLS[1].0 } else { M + 4.0 };
    let desc_w = if codes { 260.0 } else { 260.0 + COLS[1].0 - M - 4.0 };
    for line in &doc.lines {
        let parts = wrap(&line.description, desc_w, 8.0, line.is_text);
        let h = 10.0 * parts.len() as f32 + 2.0;
        l.ensure(h, doc);
        let y = l.y;
        if !line.is_text {
            if codes {
                l.pdf.text(COLS[0].0, y, 7.5, false, &line.code, Align::Left);
            }
            l.pdf.text(COLS[2].0, y, 8.0, false, &line.qty, Align::Right);
            l.pdf.text(COLS[3].0, y, 7.5, false, &line.unit, Align::Left);
            if doc.show_prices {
                l.pdf.text(COLS[4].0, y, 8.0, false, &line.price, Align::Right);
                if l.style.show_discount {
                    l.pdf.text(COLS[5].0, y, 7.5, false, &line.discount, Align::Right);
                }
                l.pdf.text(COLS[6].0, y, 8.0, false, &line.total, Align::Right);
                if l.style.show_vat {
                    l.pdf.text(COLS[7].0, y, 7.5, false, &line.vat, Align::Right);
                }
            }
        }
        for (j, p) in parts.iter().enumerate() {
            l.pdf.text(desc_x, y + j as f32 * 10.0, 8.0, line.is_text, p, Align::Left);
        }
        l.y += h;
        l.pdf.line(M, l.y - 7.0, A4_W - M, l.y - 7.0, 0.2);
    }
    l.y += 8.0;

    // riepilogo IVA e totali
    if doc.show_prices {
        let block_h = 24.0 + 11.0 * doc.vat_rows.len().max(doc.totals.len()) as f32;
        l.ensure(block_h + 10.0, doc);
        let top = l.y;
        match l.style.template {
            Template::Classic => l.pdf.rect(M, top - 10.0, 300.0, 14.0, Some(0.90), false),
            Template::Modern => l.pdf.rect_rgb(M, top - 10.0, 300.0, 14.0, l.style.accent_light()),
            Template::Essential => l.pdf.line(M, top + 4.0, M + 300.0, top + 4.0, 0.6),
        }
        l.pdf.text(M + 4.0, top, 7.5, true, "Aliquota", Align::Left);
        l.pdf.text(M + 150.0, top, 7.5, true, "Imponibile", Align::Right);
        l.pdf.text(M + 210.0, top, 7.5, true, "Imposta", Align::Right);
        l.pdf.text(M + 218.0, top, 7.5, true, "Note", Align::Left);
        let mut vy = top + 13.0;
        for (a, imp, iva, d) in &doc.vat_rows {
            l.pdf.text(M + 4.0, vy, 8.0, false, a, Align::Left);
            l.pdf.text(M + 150.0, vy, 8.0, false, imp, Align::Right);
            l.pdf.text(M + 210.0, vy, 8.0, false, iva, Align::Right);
            let short: String = d.chars().take(30).collect();
            l.pdf.text(M + 218.0, vy, 6.5, false, &short, Align::Left);
            vy += 11.0;
        }
        let tx0 = A4_W - M - 190.0;
        let mut ty = top;
        for (k, v, strong) in &doc.totals {
            if *strong {
                ty += 4.0;
            }
            let modern = *strong && l.style.template == Template::Modern;
            if *strong {
                match l.style.template {
                    Template::Classic => l.pdf.rect(tx0 - 4.0, ty - 10.0, 194.0, 15.0, Some(0.85), false),
                    Template::Modern => l.pdf.rect_rgb(tx0 - 4.0, ty - 10.0, 194.0, 15.0, l.style.accent_f()),
                    Template::Essential => l.pdf.line(tx0 - 4.0, ty - 11.0, A4_W - M, ty - 11.0, 0.8),
                }
            }
            if modern {
                l.pdf.text_rgb(tx0, ty, 9.5, true, k, Align::Left, (1.0, 1.0, 1.0));
                l.pdf.text_rgb(A4_W - M - 3.0, ty, 10.0, true, v, Align::Right, (1.0, 1.0, 1.0));
            } else {
                l.pdf.text(tx0, ty, if *strong { 9.5 } else { 8.0 }, *strong, k, Align::Left);
                l.pdf.text(A4_W - M, ty, if *strong { 10.0 } else { 8.5 }, *strong, v, Align::Right);
            }
            ty += if *strong { 16.0 } else { 11.0 };
        }
        l.y = vy.max(ty) + 10.0;
    }

    // scadenze
    if !doc.payments.is_empty() {
        l.ensure(20.0 + 10.0 * doc.payments.len() as f32, doc);
        l.pdf.text(M, l.y, 8.0, true, "SCADENZE", Align::Left);
        l.y += 11.0;
        for (data, mode, amount) in &doc.payments {
            l.pdf.text(M, l.y, 8.0, false, data, Align::Left);
            l.pdf.text(M + 70.0, l.y, 8.0, false, mode, Align::Left);
            l.pdf.text(M + 300.0, l.y, 8.0, false, amount, Align::Right);
            l.y += 10.0;
        }
        l.y += 6.0;
    }

    // note
    for n in &doc.notes {
        for p in wrap(n, A4_W - 2.0 * M, 8.0, false) {
            l.ensure(10.0, doc);
            l.pdf.text(M, l.y, 8.0, false, &p, Align::Left);
            l.y += 10.0;
        }
        l.y += 4.0;
    }

    // piè di pagina dell'ultima pagina (le precedenti lo ricevono al cambio pagina)
    l.page_footer(doc);
}

fn company_lines(c: &Company) -> Vec<String> {
    let mut v = vec![c.display_name()];
    let mut addr = c.address.clone();
    if !c.civic.is_empty() {
        addr.push_str(&format!(", {}", c.civic));
    }
    v.push(addr);
    v.push(format!("{} {} {}", c.zip, c.city, if c.province.is_empty() { String::new() } else { format!("({})", c.province) }).trim().to_string());
    let mut fisc = String::new();
    if !c.vat_number.is_empty() {
        fisc.push_str(&format!("P.IVA {}{}", if c.vat_country == "IT" { "" } else { &c.vat_country }, c.vat_number));
    }
    if !c.tax_code.is_empty() && c.tax_code != c.vat_number {
        fisc.push_str(&format!("  C.F. {}", c.tax_code));
    }
    if !fisc.is_empty() {
        v.push(fisc);
    }
    let mut contacts = Vec::new();
    if !c.phone.is_empty() {
        contacts.push(format!("Tel. {}", c.phone));
    }
    if !c.email.is_empty() {
        contacts.push(c.email.clone());
    }
    if !c.pec.is_empty() {
        contacts.push(format!("PEC {}", c.pec));
    }
    if !contacts.is_empty() {
        v.push(contacts.join("  "));
    }
    if !c.rea_number.is_empty() {
        v.push(format!(
            "REA {}-{}{}",
            c.rea_office,
            c.rea_number,
            c.share_capital.map(|cap| format!("  Cap. soc. {} i.v.", fmt_eur(cap))).unwrap_or_default()
        ));
    }
    v.retain(|s| !s.trim().is_empty());
    v
}

/// Prepara la stampa di un documento del gestionale.
pub fn from_document(company: &Company, doc: &Document, totals: &Totals) -> PrintDoc {
    let p = &doc.party;
    let mut recipient = vec![p.display_name(), p.full_address(), p.city_line()];
    if !p.vat_number.is_empty() {
        recipient.push(format!("P.IVA {}{}", if p.vat_country == "IT" || p.vat_country.is_empty() { "" } else { &p.vat_country }, p.vat_number));
    }
    if !p.tax_code.is_empty() && p.tax_code != p.vat_number {
        recipient.push(format!("C.F. {}", p.tax_code));
    }
    if doc.kind.is_fiscal() {
        if !p.sdi_code.is_empty() {
            recipient.push(format!("Codice destinatario {}", p.sdi_code));
        } else if !p.pec.is_empty() {
            recipient.push(format!("PEC {}", p.pec));
        }
    }
    recipient.retain(|s| !s.trim().is_empty());

    let mut info = Vec::new();
    if let Some(pay) = &doc.payment {
        info.push(("Pagamento".to_string(), format!("{} ({})", pay.name, describe_installments(pay))));
        if pay.show_iban && !company.iban.is_empty() {
            info.push(("IBAN".to_string(), format!("{} {}", company.iban, company.bank_name).trim().to_string()));
        }
    }
    for r in &doc.related {
        let kind = match r.kind.as_str() {
            "FATTURA" => "Rif. fattura",
            "CONTRATTO" => "Contratto",
            _ => "Vs. ordine",
        };
        let mut s = r.number.clone();
        if let Some(d) = r.date {
            s.push_str(&format!(" del {}", d.format("%d/%m/%Y")));
        }
        if !r.cig.is_empty() {
            s.push_str(&format!(" CIG {}", r.cig));
        }
        if !r.cup.is_empty() {
            s.push_str(&format!(" CUP {}", r.cup));
        }
        info.push((kind.to_string(), s));
    }
    if !doc.external_number.is_empty() {
        info.push(("Doc. fornitore".into(), doc.external_number.clone()));
    }
    if doc.kind == DocKind::StockTransfer {
        info.push(("Da magazzino".into(), if doc.warehouse.is_empty() { "predefinito".into() } else { doc.warehouse.clone() }));
        info.push(("A magazzino".into(), doc.to_warehouse.clone()));
    } else if !doc.warehouse.is_empty() && doc.kind.stock_effect() != 0 {
        info.push(("Magazzino".into(), doc.warehouse.clone()));
    }
    if let Some(t) = &doc.transport {
        if doc.kind.has_transport() {
            if !t.reason.is_empty() {
                info.push(("Causale trasp.".into(), t.reason.clone()));
            }
            if !t.by.is_empty() {
                info.push(("Trasporto a cura".into(), t.by.clone()));
            }
            if !t.carrier.is_empty() {
                info.push(("Vettore".into(), t.carrier.clone()));
            }
            if let Some(n) = t.packages {
                info.push(("Colli".into(), n.to_string()));
            }
            if let Some(w) = t.gross_weight {
                info.push(("Peso lordo".into(), format!("{} kg", fmt_it_trim(w, 0, 3))));
            }
            if !t.appearance.is_empty() {
                info.push(("Aspetto beni".into(), t.appearance.clone()));
            }
            if !t.port.is_empty() {
                info.push(("Porto".into(), t.port.clone()));
            }
            if let Some(s) = t.start {
                info.push(("Inizio trasporto".into(), s.format("%d/%m/%Y %H:%M").to_string()));
            }
        }
    }

    let show_prices = !matches!(doc.kind, DocKind::DeliveryNote | DocKind::StockTransfer | DocKind::Production);
    let cur = doc.currency_code().to_string();
    let mut lines = Vec::new();
    for lc in &totals.lines {
        let src = lc.index.and_then(|i| doc.lines.get(i));
        if let Some(v) = lc.subtotal {
            lines.push(PrintLine { description: lc.description.clone(), total: fmt_eur(v), ..Default::default() });
            continue;
        }
        if lc.is_text {
            lines.push(PrintLine { description: lc.description.clone(), is_text: true, ..Default::default() });
            continue;
        }
        let disc = src.map(|l| {
            let mut d = l.discounts.clone();
            if !doc.discount.trim().is_empty() {
                if !d.is_empty() {
                    d.push('+');
                }
                d.push_str(doc.discount.trim());
            }
            d
        });
        lines.push(PrintLine {
            // agli ordini verso il fornitore si stampa il suo codice
            code: src.map(|l| if l.supplier_code.trim().is_empty() { l.code.clone() } else { l.supplier_code.clone() }).unwrap_or_default(),
            description: src.map(|l| l.description_with_tracking()).unwrap_or_else(|| lc.description.clone()),
            qty: fmt_it_trim(lc.qty, 0, 4),
            unit: src.map(|l| l.unit.clone()).unwrap_or_default(),
            price: fmt_it_trim(src.map(|l| l.unit_price).unwrap_or(lc.unit_net), 2, 6),
            discount: disc.unwrap_or_default(),
            total: fmt_eur(lc.total),
            vat: lc.vat.label(),
            is_text: false,
        });
    }
    let _ = LineKind::Item;

    let vat_rows = totals
        .vat_rows
        .iter()
        .map(|r| {
            let label = match &r.natura {
                Some(n) => format!("{} {}", n, codes::describe(codes::NATURA, n)).chars().take(28).collect(),
                None => format!("{}%", fmt_it_trim(r.rate, 0, 2)),
            };
            let note = if r.liability == "S" { "Scissione pagamenti art. 17-ter".to_string() } else { r.legal_ref.clone() };
            (label, fmt_eur(r.taxable), fmt_eur(r.tax), note)
        })
        .collect();

    let mut tot = vec![("Imponibile".to_string(), fmt_eur(totals.taxable), false), ("IVA".to_string(), fmt_eur(totals.tax), false)];
    if let Some(pf) = &totals.pension_fund {
        tot.insert(0, (format!("Contributo {} {}%", pf.kind, fmt_it_trim(pf.rate, 0, 2)), fmt_eur(pf.amount), false));
    }
    tot.push(("Totale documento".to_string(), format!("{} {}", cur, fmt_eur(totals.total)), true));
    for w in &totals.withholdings {
        tot.push((format!("Ritenuta {} {}%", w.kind, fmt_it_trim(w.rate, 0, 2)), format!("-{}", fmt_eur(w.amount)), false));
    }
    if doc.vat_liability == "S" {
        tot.push(("IVA in scissione".into(), format!("-{}", fmt_eur(totals.tax)), false));
    }
    if totals.net_to_pay != totals.total {
        tot.push(("Netto a pagare".to_string(), format!("{} {}", cur, fmt_eur(totals.net_to_pay)), true));
    }

    let payments = doc
        .due_dates
        .iter()
        .map(|d| (d.date.format("%d/%m/%Y").to_string(), codes::describe(codes::MODALITA_PAGAMENTO, &d.mp_code).to_string(), fmt_eur(d.amount)))
        .collect();

    let mut notes = Vec::new();
    if let Some(s) = totals.stamp {
        notes.push(format!("Imposta di bollo assolta in modo virtuale ai sensi del DM 17/06/2014 (EUR {}).", fmt_eur(s)));
    }
    if !doc.causale.trim().is_empty() {
        notes.push(doc.causale.clone());
    }
    if !doc.notes.trim().is_empty() {
        notes.push(doc.notes.clone());
    }
    if !doc.intent_protocol.trim().is_empty() {
        let d = doc.party.intents.iter().find(|i| i.protocol == doc.intent_protocol).and_then(|i| i.date);
        notes.push(format!(
            "Operazione non imponibile art. 8 c.1 lett. c DPR 633/72 - dichiarazione d'intento prot. {}{}",
            doc.intent_protocol.trim(),
            d.map(|d| format!(" del {}", d.format("%d/%m/%Y"))).unwrap_or_default()
        ));
    }
    if doc.is_foreign_currency() {
        notes.push(format!(
            "Importi in {}. Cambio 1 EUR = {} {}. Controvalore in euro: imponibile EUR {}, IVA EUR {}, totale EUR {}.",
            cur,
            doc.exchange_rate.normalize(),
            cur,
            fmt_eur(doc.to_eur(totals.taxable)),
            fmt_eur(doc.to_eur(totals.tax)),
            fmt_eur(doc.to_eur(totals.total))
        ));
    }
    if doc.kind.is_fiscal() {
        notes.push("Copia di cortesia: l'originale è la fattura elettronica trasmessa tramite il Sistema di Interscambio.".into());
    }

    PrintDoc {
        title: doc.kind.label().to_string(),
        number: doc.number_label(),
        date: doc.date.format("%d/%m/%Y").to_string(),
        issuer: company_lines(company),
        recipient_title: if doc.kind.is_purchase() { "FORNITORE".into() } else { "SPETT.LE".into() },
        recipient,
        destination: doc.destination.lines().map(|s| s.to_string()).filter(|s| !s.trim().is_empty()).collect(),
        info,
        lines,
        vat_rows,
        totals: tot,
        payments,
        notes,
        footer: company.invoice_footer.clone(),
        show_prices,
        style: PrintStyle::default(),
        watermark: match doc.status {
            openfatt_core::model::DocStatus::Draft => Some("BOZZA - documento non emesso".into()),
            openfatt_core::model::DocStatus::Cancelled => Some("ANNULLATO".into()),
            _ => None,
        },
    }
}

/// Prepara la stampa leggibile di una fattura elettronica (es. ricevuta da un fornitore).
pub fn from_einvoice(f: &openfatt_fatturapa::FatturaElettronica, body: usize) -> PrintDoc {
    use openfatt_fatturapa::model::*;
    let b = &f.bodies[body.min(f.bodies.len().saturating_sub(1))];
    let ced = &f.header.cedente;
    let cess = &f.header.cessionario;
    let addr = |i: &Indirizzo| {
        vec![
            format!("{}{}", i.indirizzo, i.numero_civico.as_ref().map(|c| format!(", {}", c)).unwrap_or_default()),
            format!("{} {} {} {}", i.cap, i.comune, i.provincia.as_ref().map(|p| format!("({})", p)).unwrap_or_default(), if i.nazione == "IT" { "" } else { &i.nazione }).trim().to_string(),
        ]
    };
    let mut issuer = vec![ced.anagrafica.display()];
    issuer.extend(addr(&ced.sede));
    issuer.push(format!(
        "P.IVA {}{}{}",
        ced.id_fiscale_iva.paese,
        ced.id_fiscale_iva.codice,
        ced.codice_fiscale.as_ref().map(|c| format!("  C.F. {}", c)).unwrap_or_default()
    ));
    issuer.push(format!("Regime fiscale {} {}", ced.regime_fiscale, codes::describe(codes::REGIME_FISCALE, &ced.regime_fiscale)));
    let mut recipient = vec![cess.anagrafica.display()];
    recipient.extend(addr(&cess.sede));
    if let Some(id) = &cess.id_fiscale_iva {
        recipient.push(format!("P.IVA {}{}", id.paese, id.codice));
    }
    if let Some(cf) = &cess.codice_fiscale {
        recipient.push(format!("C.F. {}", cf));
    }
    let d = &b.dati_generali.documento;
    let mut info = vec![
        ("Tipo documento".to_string(), format!("{} {}", d.tipo_documento, codes::describe(codes::TIPO_DOCUMENTO, &d.tipo_documento))),
        ("Trasmissione".to_string(), format!("{} - prog. {} - dest. {}", f.versione, f.header.dati_trasmissione.progressivo_invio, f.header.dati_trasmissione.codice_destinatario)),
    ];
    for o in &b.dati_generali.ordini_acquisto {
        info.push(("Ordine".into(), format!("{} {}", o.id_documento, o.data.map(|x| x.format("%d/%m/%Y").to_string()).unwrap_or_default())));
    }
    for ddt in &b.dati_generali.ddt {
        info.push(("DDT".into(), format!("{} del {}", ddt.numero_ddt, ddt.data_ddt.format("%d/%m/%Y"))));
    }
    for fc in &b.dati_generali.fatture_collegate {
        info.push(("Fattura collegata".into(), format!("{} {}", fc.id_documento, fc.data.map(|x| x.format("%d/%m/%Y").to_string()).unwrap_or_default())));
    }
    let lines = b
        .linee
        .iter()
        .map(|l| {
            let disc: Vec<String> = l
                .sconto_maggiorazione
                .iter()
                .map(|s| {
                    let sign = if s.tipo == "MG" { "+" } else { "" };
                    match (s.percentuale, s.importo) {
                        (Some(p), _) => format!("{}{}", sign, fmt_it_trim(p, 0, 2)),
                        (None, Some(i)) => format!("{}{}€", sign, fmt_it_trim(i, 2, 4)),
                        _ => String::new(),
                    }
                })
                .collect();
            PrintLine {
                code: l.codice_articolo.first().map(|c| c.valore.clone()).unwrap_or_default(),
                description: l.descrizione.clone(),
                qty: l.quantita.map(|q| fmt_it_trim(q, 0, 4)).unwrap_or_default(),
                unit: l.unita_misura.clone().unwrap_or_default(),
                price: fmt_it_trim(l.prezzo_unitario, 2, 6),
                discount: disc.join("+"),
                total: fmt_eur(l.prezzo_totale),
                vat: match &l.natura {
                    Some(n) => n.clone(),
                    None => format!("{}%", fmt_it_trim(l.aliquota_iva, 0, 2)),
                },
                is_text: false,
            }
        })
        .collect();
    let vat_rows = b
        .riepilogo
        .iter()
        .map(|r| {
            (
                r.natura.clone().unwrap_or_else(|| format!("{}%", fmt_it_trim(r.aliquota_iva, 0, 2))),
                fmt_eur(r.imponibile_importo),
                fmt_eur(r.imposta),
                r.riferimento_normativo.clone().unwrap_or_else(|| r.esigibilita_iva.clone().map(|e| format!("Esigibilità {}", e)).unwrap_or_default()),
            )
        })
        .collect();
    let taxable: Decimal = b.riepilogo.iter().map(|r| r.imponibile_importo).sum();
    let tax: Decimal = b.riepilogo.iter().map(|r| r.imposta).sum();
    let total = openfatt_fatturapa::to_domain::declared_total(b);
    let mut totals = vec![("Imponibile".into(), fmt_eur(taxable), false), ("IVA".into(), fmt_eur(tax), false), ("Totale documento".into(), format!("EUR {}", fmt_eur(total)), true)];
    for r in &d.dati_ritenuta {
        totals.push((format!("Ritenuta {} {}%", r.tipo_ritenuta, fmt_it_trim(r.aliquota_ritenuta, 0, 2)), format!("-{}", fmt_eur(r.importo_ritenuta)), false));
    }
    let payments = b
        .pagamenti
        .iter()
        .flat_map(|p| p.dettagli.iter())
        .map(|dp| {
            (
                dp.data_scadenza.map(|x| x.format("%d/%m/%Y").to_string()).unwrap_or_else(|| "-".into()),
                format!(
                    "{}{}",
                    codes::describe(codes::MODALITA_PAGAMENTO, &dp.modalita_pagamento),
                    dp.iban.as_ref().map(|i| format!(" IBAN {}", i)).unwrap_or_default()
                ),
                fmt_eur(dp.importo),
            )
        })
        .collect();
    let mut notes: Vec<String> = d.causale.clone();
    if let Some(bo) = &d.dati_bollo {
        notes.push(format!("Bollo virtuale: {}", bo.importo_bollo.map(fmt_eur).unwrap_or_default()));
    }
    if !b.allegati.is_empty() {
        notes.push(format!("Allegati: {}", b.allegati.iter().map(|a| a.nome.clone()).collect::<Vec<_>>().join(", ")));
    }
    PrintDoc {
        title: codes::describe(codes::TIPO_DOCUMENTO, &d.tipo_documento).to_string(),
        number: d.numero.clone(),
        date: d.data.map(|x| x.format("%d/%m/%Y").to_string()).unwrap_or_default(),
        issuer,
        recipient_title: "CESSIONARIO / COMMITTENTE".into(),
        recipient,
        destination: vec![],
        info,
        lines,
        vat_rows,
        totals,
        payments,
        notes,
        footer: "Rappresentazione leggibile della fattura elettronica".into(),
        show_prices: true,
        watermark: None,
        style: PrintStyle::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_valid_pdf_multi_page() {
        let mut d = PrintDoc { title: "Fattura".into(), number: "1".into(), date: "01/01/2026".into(), show_prices: true, ..Default::default() };
        d.issuer = vec!["Azienda àèìòù €".into(), "Via (prova) \\ 1".into()];
        d.recipient = vec!["Cliente".into()];
        for i in 0..120 {
            d.lines.push(PrintLine { code: format!("A{}", i), description: "Descrizione molto lunga che deve andare a capo più volte per verificare il ritorno a capo automatico".into(), qty: "1".into(), price: "1,00".into(), total: "1,00".into(), vat: "22%".into(), ..Default::default() });
        }
        d.totals = vec![("Totale".into(), "120,00".into(), true)];
        let pdf = render(&d);
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.matches("/Type /Page ").count() > 2);
        // le parentesi nel testo sono protette
        assert!(s.contains("\\(prova\\)"));
    }

    #[test]
    fn wrapping() {
        let w = wrap("uno due tre quattro cinque sei sette otto nove dieci", 60.0, 8.0, false);
        assert!(w.len() > 1);
        assert!(w.iter().all(|l| writer::text_width(l, 8.0, false) <= 60.0));
    }
}
