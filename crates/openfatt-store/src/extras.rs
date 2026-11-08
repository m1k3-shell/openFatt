//! Funzioni sulle righe dei documenti: documenti ricorrenti, ultimi prezzi praticati,
//! allegati e foto, carico degli articoli dalla fattura elettronica del fornitore,
//! dichiarazioni d'intento e controllo del plafond.

use chrono::{Datelike, NaiveDate};
use rusqlite::{params, OptionalExtension};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use openfatt_core::calc;
use openfatt_core::model::{DocKind, DocLine, DocStatus, Document, LineKind, Product, Tracking};

use crate::{date_from_sql, date_to_sql, now_sql, Result, Store, StoreError};

// ---------------------------------------------------------------------------
// Documenti ricorrenti
// ---------------------------------------------------------------------------

/// Pianificazione di un documento che si ripete (canoni, abbonamenti, affitti).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Recurrence {
    pub id: i64,
    /// Documento modello (resta in bozza e non viene emesso)
    pub template_id: i64,
    pub description: String,
    /// Ogni quanti mesi: 1 mensile, 2 bimestrale, 3 trimestrale, 6 semestrale, 12 annuale
    pub every_months: u32,
    pub next_date: NaiveDate,
    pub end_date: Option<NaiveDate>,
    /// Emette subito i documenti generati (altrimenti restano in bozza)
    pub issue: bool,
    pub active: bool,
    pub last_generated: Option<NaiveDate>,
    pub generated: u32,
}

impl Default for Recurrence {
    fn default() -> Self {
        Recurrence {
            id: 0,
            template_id: 0,
            description: String::new(),
            every_months: 1,
            next_date: chrono::Local::now().date_naive(),
            end_date: None,
            issue: false,
            active: true,
            last_generated: None,
            generated: 0,
        }
    }
}

const MESI: [&str; 12] = ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"];

/// Aggiunge mesi a una data mantenendo il giorno (o l'ultimo del mese se più corto).
pub fn add_months(d: NaiveDate, months: u32) -> NaiveDate {
    let total = d.year() * 12 + d.month0() as i32 + months as i32;
    let (y, m) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    let mut day = d.day();
    loop {
        if let Some(x) = NaiveDate::from_ymd_opt(y, m, day) {
            return x;
        }
        day -= 1;
    }
}

/// Sostituisce i segnaposto del periodo: {mese}, {anno}, {periodo}, {mese_succ}.
pub fn period_text(text: &str, date: NaiveDate, every_months: u32) -> String {
    let mese = MESI[date.month0() as usize];
    let periodo = if every_months <= 1 {
        format!("{} {}", mese, date.year())
    } else {
        let end = add_months(date, every_months - 1);
        format!("{} {} - {} {}", mese, date.year(), MESI[end.month0() as usize], end.year())
    };
    let next = add_months(date, 1);
    text.replace("{mese}", mese)
        .replace("{anno}", &date.year().to_string())
        .replace("{periodo}", &periodo)
        .replace("{mese_succ}", MESI[next.month0() as usize])
}

/// Ultimo prezzo praticato a un cliente per un articolo.
#[derive(Debug, Clone, PartialEq)]
pub struct LastPrice {
    pub document_id: i64,
    pub title: String,
    pub date: NaiveDate,
    pub qty: Decimal,
    pub unit_price: Decimal,
    pub discounts: String,
    /// Prezzo netto unitario dopo gli sconti (stessa base IVA del documento)
    pub net: Decimal,
    pub prices_include_vat: bool,
}

/// Esito del carico articoli da una fattura ricevuta.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProductImport {
    pub created: usize,
    pub updated: usize,
    pub skipped: usize,
}

/// Allegato a un documento, soggetto o articolo (anche foto dell'articolo).
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub id: i64,
    pub entity: String,
    pub entity_id: i64,
    pub name: String,
    pub mime: String,
    pub size: i64,
    pub created_at: String,
}

/// Uso del plafond di una dichiarazione d'intento.
#[derive(Debug, Clone, PartialEq)]
pub struct IntentUsage {
    pub protocol: String,
    pub limit: Decimal,
    pub used: Decimal,
    pub documents: usize,
}

impl IntentUsage {
    pub fn residual(&self) -> Option<Decimal> {
        if self.limit.is_zero() {
            None
        } else {
            Some(self.limit - self.used)
        }
    }
}

pub const MAX_ATTACHMENT: usize = 20 * 1024 * 1024;

impl Store {
    pub fn recurrences(&self) -> Result<Vec<Recurrence>> {
        let mut st = self.conn().prepare("SELECT id, template_id, data FROM recurring ORDER BY next_date")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, t, data) = row?;
            let mut x: Recurrence = serde_json::from_str(&data)?;
            x.id = id;
            x.template_id = t;
            out.push(x);
        }
        Ok(out)
    }

    pub fn save_recurrence(&self, r: &Recurrence) -> Result<Recurrence> {
        if r.every_months == 0 || r.every_months > 24 {
            return Err(StoreError::Invalid("Periodicità non valida (da 1 a 24 mesi)".into()));
        }
        let t = self.document(r.template_id)?;
        if t.status != DocStatus::Draft {
            return Err(StoreError::Invalid("Il modello di un documento ricorrente deve restare in bozza".into()));
        }
        let mut r = r.clone();
        let data = serde_json::to_string(&r)?;
        if r.id == 0 {
            self.conn().execute(
                "INSERT INTO recurring(template_id, next_date, active, data) VALUES (?1, ?2, ?3, ?4)",
                params![r.template_id, date_to_sql(r.next_date), r.active as i64, data],
            )?;
            r.id = self.conn().last_insert_rowid();
        } else {
            self.conn().execute(
                "UPDATE recurring SET template_id=?1, next_date=?2, active=?3, data=?4 WHERE id=?5",
                params![r.template_id, date_to_sql(r.next_date), r.active as i64, data, r.id],
            )?;
        }
        // il modello resta fuori da numerazione e scadenze
        self.conn().execute("UPDATE documents SET number = NULL, data = json_set(data, '$.number', json('null')) WHERE id = ?1", [r.template_id])?;
        self.conn().execute("DELETE FROM due_dates WHERE document_id = ?1", [r.template_id])?;
        self.audit("recurring", r.id, "save", &r.description)?;
        Ok(r)
    }

    pub fn delete_recurrence(&self, id: i64) -> Result<()> {
        self.conn().execute("DELETE FROM recurring WHERE id = ?1", [id])?;
        self.audit("recurring", id, "delete", "")?;
        Ok(())
    }

    /// Ricorrenze con documenti da generare entro la data indicata.
    pub fn due_recurrences(&self, today: NaiveDate) -> Result<Vec<Recurrence>> {
        Ok(self.recurrences()?.into_iter().filter(|r| r.active && r.next_date <= today && r.end_date.map(|e| r.next_date <= e).unwrap_or(true)).collect())
    }

    /// Genera i documenti ricorrenti scaduti fino a oggi. Restituisce gli id creati.
    pub fn generate_recurring(&self, today: NaiveDate) -> Result<Vec<i64>> {
        let mut created = Vec::new();
        for mut r in self.due_recurrences(today)? {
            let template = self.document(r.template_id)?;
            let tx = self.begin()?;
            while r.next_date <= today && r.end_date.map(|e| r.next_date <= e).unwrap_or(true) {
                let mut d = template.clone();
                d.id = 0;
                d.version = 0;
                d.number = None;
                d.status = DocStatus::Draft;
                d.date = r.next_date;
                d.year = r.next_date.year();
                d.created_at = None;
                d.due_dates.clear();
                d.notes = period_text(&d.notes, r.next_date, r.every_months);
                d.causale = period_text(&d.causale, r.next_date, r.every_months);
                for l in d.lines.iter_mut() {
                    l.id = 0;
                    l.source_doc_id = None;
                    l.source_line_id = None;
                    l.description = period_text(&l.description, r.next_date, r.every_months);
                    if l.period_start.is_some() || l.period_end.is_some() {
                        l.period_start = Some(r.next_date);
                        l.period_end = Some(add_months(r.next_date, r.every_months).pred_opt().unwrap_or(r.next_date));
                    }
                }
                let saved = self.save_document(&d)?;
                let id = if r.issue { self.issue_document(saved.id)?.id } else { saved.id };
                created.push(id);
                r.last_generated = Some(r.next_date);
                r.generated += 1;
                r.next_date = add_months(r.next_date, r.every_months);
            }
            if r.end_date.map(|e| r.next_date > e).unwrap_or(false) {
                r.active = false;
            }
            self.save_recurrence(&r)?;
            tx.commit()?;
        }
        Ok(created)
    }

    // ------------------------------------------------------------------
    // Ultimi prezzi praticati
    // ------------------------------------------------------------------

    /// Ultimi prezzi di vendita (o di acquisto) dell'articolo al soggetto, dal più recente.
    pub fn last_prices(&self, party_id: i64, product_id: i64, purchase: bool, limit: usize) -> Result<Vec<LastPrice>> {
        let mut st = self.conn().prepare(
            "SELECT DISTINCT d.id, d.kind, d.date FROM doc_lines l JOIN documents d ON d.id = l.document_id
             WHERE l.product_id = ?1 AND d.party_id = ?2 AND d.status = 'EMESSO'
             ORDER BY d.date DESC, d.id DESC LIMIT 60",
        )?;
        let ids: Vec<(i64, String)> = st.query_map(params![product_id, party_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
        let mut out = Vec::new();
        for (id, kind) in ids {
            let Some(k) = DocKind::from_code(&kind) else { continue };
            if k.is_purchase() != purchase || k == DocKind::Quote || k.is_internal() || k.is_credit() {
                continue;
            }
            let doc = self.document(id)?;
            let doc_disc = calc::parse_discounts(&doc.discount).unwrap_or_default();
            for l in doc.lines.iter().filter(|l| l.product_id == Some(product_id) && l.kind == LineKind::Item) {
                let mut disc = calc::parse_discounts(&l.discounts).unwrap_or_default();
                disc.extend(doc_disc.iter().copied());
                out.push(LastPrice {
                    document_id: id,
                    title: doc.title(),
                    date: doc.date,
                    qty: l.qty,
                    unit_price: l.unit_price,
                    discounts: l.discounts.clone(),
                    net: openfatt_core::money::round_dp(calc::apply_cascade(l.unit_price, &disc), 4),
                    prices_include_vat: doc.prices_include_vat,
                });
            }
            if out.len() >= limit {
                break;
            }
        }
        out.truncate(limit);
        Ok(out)
    }

    // ------------------------------------------------------------------
    // Allegati e foto
    // ------------------------------------------------------------------

    pub fn add_attachment(&self, entity: &str, entity_id: i64, name: &str, content: &[u8]) -> Result<i64> {
        if content.len() > MAX_ATTACHMENT {
            return Err(StoreError::Invalid(format!("File troppo grande ({} MB, massimo 20 MB)", content.len() / 1_048_576)));
        }
        let mime = mime_of(name);
        self.conn().execute(
            "INSERT INTO attachments(entity, entity_id, name, mime, size, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![entity, entity_id, name, mime, content.len() as i64, content, now_sql()],
        )?;
        let id = self.conn().last_insert_rowid();
        self.audit("attachment", id, "add", &format!("{} {} {}", entity, entity_id, name))?;
        Ok(id)
    }

    pub fn attachments(&self, entity: &str, entity_id: i64) -> Result<Vec<Attachment>> {
        let mut st = self.conn().prepare("SELECT id, name, mime, size, created_at FROM attachments WHERE entity = ?1 AND entity_id = ?2 ORDER BY id")?;
        let rows = st.query_map(params![entity, entity_id], |r| {
            Ok(Attachment { id: r.get(0)?, entity: entity.to_string(), entity_id, name: r.get(1)?, mime: r.get(2)?, size: r.get(3)?, created_at: r.get(4)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn attachment_content(&self, id: i64) -> Result<(String, Vec<u8>)> {
        self.conn()
            .query_row("SELECT name, content FROM attachments WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .ok_or_else(|| StoreError::NotFound("allegato".into()))
    }

    pub fn delete_attachment(&self, id: i64) -> Result<()> {
        self.conn().execute("DELETE FROM attachments WHERE id = ?1", [id])?;
        self.audit("attachment", id, "delete", "")?;
        Ok(())
    }

    /// Foto dell'articolo (la prima immagine allegata come "product_photo").
    pub fn product_photo(&self, product_id: i64) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn()
            .query_row("SELECT content FROM attachments WHERE entity = 'product_photo' AND entity_id = ?1 ORDER BY id DESC LIMIT 1", [product_id], |r| r.get(0))
            .optional()?)
    }

    pub fn set_product_photo(&self, product_id: i64, name: &str, content: Option<&[u8]>) -> Result<()> {
        let tx = self.begin()?;
        self.conn().execute("DELETE FROM attachments WHERE entity = 'product_photo' AND entity_id = ?1", [product_id])?;
        if let Some(c) = content {
            self.add_attachment("product_photo", product_id, name, c)?;
        }
        tx.commit()
    }

    // ------------------------------------------------------------------
    // Carico articoli dalla fattura elettronica del fornitore
    // ------------------------------------------------------------------

    /// Crea o aggiorna gli articoli delle righe di un documento di acquisto (codice del
    /// fornitore, descrizione, costo, IVA) e collega le righe agli articoli. Il codice
    /// interno dei nuovi articoli è il codice del fornitore, preceduto dal prefisso indicato.
    pub fn import_products_from_purchase(&self, doc_id: i64, prefix: &str, update_cost: bool) -> Result<ProductImport> {
        let mut doc = self.document(doc_id)?;
        if !doc.kind.is_purchase() {
            return Err(StoreError::Invalid("Il documento non è un acquisto".into()));
        }
        let supplier = doc.party_id;
        let mut res = ProductImport::default();
        let tx = self.begin()?;
        let doc_disc = calc::parse_discounts(&doc.discount).unwrap_or_default();
        for l in doc.lines.iter_mut() {
            if l.kind != LineKind::Item || l.description.trim().is_empty() || l.qty.is_zero() {
                res.skipped += 1;
                continue;
            }
            let code = l.code.trim().to_string();
            let mut disc = calc::parse_discounts(&l.discounts).unwrap_or_default();
            disc.extend(doc_disc.iter().copied());
            let mut cost = openfatt_core::money::round_dp(calc::apply_cascade(l.unit_price, &disc), 4);
            if doc.prices_include_vat {
                cost = openfatt_core::money::round_dp(cost / (Decimal::ONE + l.vat.rate / Decimal::ONE_HUNDRED), 4);
            }
            let existing = match l.product_id {
                Some(id) => self.product(id)?,
                None => self.product_by_supplier_code(supplier, &code)?,
            };
            match existing {
                Some(mut p) => {
                    let mut changed = false;
                    if update_cost && cost > Decimal::ZERO && p.cost != cost {
                        p.cost = cost;
                        changed = true;
                    }
                    if p.supplier_id.is_none() && supplier.is_some() {
                        p.supplier_id = supplier;
                        changed = true;
                    }
                    if p.supplier_code.is_empty() && !code.is_empty() {
                        p.supplier_code = code.clone();
                        changed = true;
                    }
                    if changed {
                        p = self.save_product(&p)?;
                        res.updated += 1;
                    } else {
                        res.skipped += 1;
                    }
                    if p.variants.is_empty() && p.tracking != Tracking::Serial {
                        l.product_id = Some(p.id);
                    }
                }
                None => {
                    if code.is_empty() {
                        res.skipped += 1;
                        continue;
                    }
                    let mut new_code = format!("{}{}", prefix.trim(), code);
                    let mut n = 2;
                    while self.product_by_code(&new_code)?.is_some() {
                        new_code = format!("{}{}-{}", prefix.trim(), code, n);
                        n += 1;
                    }
                    let p = Product {
                        code: new_code,
                        description: l.description.trim().to_string(),
                        unit: if l.unit.trim().is_empty() { "pz".into() } else { l.unit.trim().to_lowercase() },
                        cost,
                        vat_id: l.vat.vat_id,
                        supplier_id: supplier,
                        supplier_code: code.clone(),
                        ..Default::default()
                    };
                    let p = self.save_product(&p)?;
                    l.product_id = Some(p.id);
                    res.created += 1;
                }
            }
        }
        // collega le righe: un documento emesso rigenera il carico di magazzino
        let lock = self.document_lock_reason(doc.id)?;
        if lock.is_none() {
            self.save_document(&doc)?;
        } else {
            let data = {
                let mut stored = doc.clone();
                stored.due_dates.clear();
                serde_json::to_string(&stored)?
            };
            self.conn().execute("UPDATE documents SET data = ?1 WHERE id = ?2", params![data, doc.id])?;
            for l in &doc.lines {
                self.conn().execute("UPDATE doc_lines SET product_id = ?1 WHERE id = ?2", params![l.product_id, l.id])?;
            }
        }
        self.audit("document", doc.id, "import_products", &format!("{} nuovi, {} aggiornati", res.created, res.updated))?;
        tx.commit()?;
        Ok(res)
    }

    pub fn product_by_supplier_code(&self, supplier: Option<i64>, code: &str) -> Result<Option<Product>> {
        if code.trim().is_empty() {
            return Ok(None);
        }
        let mut st = self.conn().prepare("SELECT id, data FROM products WHERE json_extract(data, '$.supplier_code') = ?1")?;
        let rows: Vec<(i64, String)> = st.query_map([code.trim()], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
        for (id, _) in &rows {
            if let Some(p) = self.product(*id)? {
                if supplier.is_none() || p.supplier_id == supplier || p.supplier_id.is_none() {
                    return Ok(Some(p));
                }
            }
        }
        self.product_by_code(code)
    }

    // ------------------------------------------------------------------
    // Dichiarazioni d'intento
    // ------------------------------------------------------------------

    /// Importo già fatturato in esenzione (N3.5) con la dichiarazione indicata, esclusi i
    /// documenti `exclude` (quello in modifica). Le note di credito riducono l'utilizzo.
    pub fn intent_usage(&self, party_id: i64, protocol: &str, exclude: Option<i64>) -> Result<IntentUsage> {
        let party = self.party(party_id)?.ok_or_else(|| StoreError::NotFound("soggetto".into()))?;
        let limit = party.intents.iter().find(|i| i.protocol == protocol).map(|i| i.limit).unwrap_or_default();
        let mut st = self.conn().prepare(
            "SELECT id FROM documents WHERE party_id = ?1 AND status = 'EMESSO' AND json_extract(data, '$.intent_protocol') = ?2",
        )?;
        let ids: Vec<i64> = st.query_map(params![party_id, protocol], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
        let mut used = Decimal::ZERO;
        let mut n = 0;
        for id in ids {
            if Some(id) == exclude {
                continue;
            }
            let d = self.document(id)?;
            if !d.kind.is_fiscal() {
                continue;
            }
            used += intent_amount(&d);
            n += 1;
        }
        Ok(IntentUsage { protocol: protocol.into(), limit, used, documents: n })
    }

    /// Righe di produzione: (prodotto finito, quantità) -> scarichi dei componenti.
    pub(crate) fn production_components(&self, product: &Product, qty: Decimal) -> Result<Vec<(Product, Decimal)>> {
        let mut out = Vec::new();
        for c in &product.components {
            if let Some(p) = self.product(c.product_id)? {
                out.push((p, c.qty * qty));
            }
        }
        Ok(out)
    }

    /// Costo di un prodotto con distinta base: somma dei costi dei componenti (ricorsiva).
    pub fn bom_cost(&self, product: &Product) -> Result<Decimal> {
        fn walk(s: &Store, p: &Product, depth: u32) -> Result<Decimal> {
            if depth > 8 || p.components.is_empty() {
                return Ok(p.cost);
            }
            let mut t = Decimal::ZERO;
            for c in &p.components {
                if let Some(x) = s.product(c.product_id)? {
                    t += walk(s, &x, depth + 1)? * c.qty;
                }
            }
            Ok(openfatt_core::money::round_dp(t, 4))
        }
        walk(self, product, 0)
    }

    /// Quante unità del prodotto finito si possono produrre con le giacenze dei componenti.
    pub fn producible(&self, product: &Product) -> Result<Option<Decimal>> {
        let mut best: Option<Decimal> = None;
        for c in &product.components {
            if c.qty <= Decimal::ZERO {
                continue;
            }
            let Some(p) = self.product(c.product_id)? else { continue };
            if !p.manage_stock {
                continue;
            }
            let n = (self.stock_of(p.id)? / c.qty).floor();
            best = Some(best.map(|b| b.min(n)).unwrap_or(n));
        }
        Ok(best.map(|b| b.max(Decimal::ZERO)))
    }

    /// Prima data utile (per le ricorrenze o i report): primo documento in archivio.
    pub fn first_document_date(&self) -> Result<Option<NaiveDate>> {
        let s: Option<String> = self.conn().query_row("SELECT MIN(date) FROM documents", [], |r| r.get(0))?;
        Ok(s.map(|s| date_from_sql(&s)))
    }
}

/// Imponibile N3.5 di un documento (negativo per le note di credito).
pub fn intent_amount(d: &Document) -> Decimal {
    let Ok(t) = calc::compute(d) else { return Decimal::ZERO };
    let v: Decimal = t.vat_rows.iter().filter(|r| r.natura.as_deref() == Some("N3.5")).map(|r| r.taxable).sum();
    let v = d.to_eur(v);
    if d.kind.is_credit() {
        -v
    } else {
        v
    }
}

/// Riga copiata da un altro documento: perde i riferimenti d'origine.
pub fn copy_line(l: &DocLine) -> DocLine {
    let mut c = l.clone();
    c.id = 0;
    c.source_doc_id = None;
    c.source_line_id = None;
    c.serials.clear();
    c.arrived_qty = Decimal::ZERO;
    c
}

pub fn mime_of(name: &str) -> &'static str {
    let n = name.to_lowercase();
    if n.ends_with(".pdf") {
        "application/pdf"
    } else if n.ends_with(".png") {
        "image/png"
    } else if n.ends_with(".jpg") || n.ends_with(".jpeg") {
        "image/jpeg"
    } else if n.ends_with(".xml") {
        "application/xml"
    } else if n.ends_with(".p7m") {
        "application/pkcs7-mime"
    } else if n.ends_with(".tif") || n.ends_with(".tiff") {
        "image/tiff"
    } else if n.ends_with(".txt") || n.ends_with(".csv") {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::model::*;
    use openfatt_core::dec;

    fn setup() -> (Store, Party, Product) {
        let s = Store::open_in_memory().unwrap();
        let v = s.vat_codes().unwrap().into_iter().find(|v| v.rate == dec!(22)).unwrap();
        let c = s.save_party(&Party { name: "Cliente".into(), vat_number: "01234567890".into(), city: "Roma".into(), ..Default::default() }).unwrap();
        let p = s.save_product(&Product { code: "A".into(), description: "Canone".into(), price: dec!(100), vat_id: Some(v.id), is_service: true, manage_stock: false, ..Default::default() }).unwrap();
        (s, c, p)
    }

    #[test]
    fn months_and_period_text() {
        let d = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        assert_eq!(add_months(d, 1), NaiveDate::from_ymd_opt(2026, 2, 28).unwrap());
        assert_eq!(add_months(d, 13), NaiveDate::from_ymd_opt(2027, 2, 28).unwrap());
        assert_eq!(period_text("Canone {periodo}", NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), 3), "Canone ottobre 2026 - dicembre 2026");
        assert_eq!(period_text("{mese} {anno}", NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(), 1), "marzo 2026");
    }

    #[test]
    fn recurring_documents_are_generated_until_today() {
        let (s, c, p) = setup();
        let v = s.vat_codes().unwrap().into_iter().find(|v| v.rate == dec!(22)).unwrap();
        let mut t = Document::new(DocKind::Invoice);
        t.party_id = Some(c.id);
        t.lines.push(DocLine { product_id: Some(p.id), description: "Canone di assistenza {mese} {anno}".into(), unit_price: dec!(100), vat: VatRef::from_code(&v), ..Default::default() });
        let t = s.save_document(&t).unwrap();
        let start = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
        let r = s.save_recurrence(&Recurrence { template_id: t.id, description: "Assistenza".into(), every_months: 1, next_date: start, issue: true, end_date: Some(NaiveDate::from_ymd_opt(2026, 10, 31).unwrap()), ..Default::default() }).unwrap();
        let ids = s.generate_recurring(NaiveDate::from_ymd_opt(2026, 11, 7).unwrap()).unwrap();
        assert_eq!(ids.len(), 3);
        let docs: Vec<Document> = ids.iter().map(|i| s.document(*i).unwrap()).collect();
        assert!(docs.iter().all(|d| d.status == DocStatus::Issued && d.number.is_some()));
        assert_eq!(docs[0].lines[0].description, "Canone di assistenza agosto 2026");
        assert_eq!(docs[2].lines[0].description, "Canone di assistenza ottobre 2026");
        let r2 = s.recurrences().unwrap().into_iter().find(|x| x.id == r.id).unwrap();
        assert!(!r2.active, "terminata dopo la data di fine");
        assert_eq!(r2.generated, 3);
        // nessun doppione a una seconda esecuzione
        assert!(s.generate_recurring(NaiveDate::from_ymd_opt(2026, 11, 7).unwrap()).unwrap().is_empty());
        // il modello emesso non è ammesso
        assert!(s.issue_document(t.id).is_ok());
        assert!(s.save_recurrence(&Recurrence { template_id: t.id, ..Default::default() }).is_err());
    }

    #[test]
    fn last_prices_for_customer() {
        let (s, c, p) = setup();
        let v = s.vat_codes().unwrap().into_iter().find(|v| v.rate == dec!(22)).unwrap();
        for (day, price, disc) in [(1, dec!(100), ""), (5, dec!(95), "10")] {
            let mut d = Document::new(DocKind::Invoice);
            d.date = NaiveDate::from_ymd_opt(2026, 3, day).unwrap();
            d.party_id = Some(c.id);
            d.lines.push(DocLine { product_id: Some(p.id), description: "x".into(), unit_price: price, discounts: disc.into(), vat: VatRef::from_code(&v), ..Default::default() });
            let d = s.save_document(&d).unwrap();
            s.issue_document(d.id).unwrap();
        }
        let lp = s.last_prices(c.id, p.id, false, 5).unwrap();
        assert_eq!(lp.len(), 2);
        assert_eq!(lp[0].unit_price, dec!(95));
        assert_eq!(lp[0].net, dec!(85.5));
        assert!(s.last_prices(c.id, p.id, true, 5).unwrap().is_empty());
    }

    #[test]
    fn attachments_and_photos() {
        let (s, _c, p) = setup();
        let id = s.add_attachment("document", 1, "scansione.pdf", b"%PDF-1.4").unwrap();
        assert_eq!(s.attachments("document", 1).unwrap()[0].mime, "application/pdf");
        assert_eq!(s.attachment_content(id).unwrap().1, b"%PDF-1.4");
        s.set_product_photo(p.id, "foto.png", Some(b"png")).unwrap();
        s.set_product_photo(p.id, "foto2.png", Some(b"png2")).unwrap();
        assert_eq!(s.product_photo(p.id).unwrap().unwrap(), b"png2");
        s.set_product_photo(p.id, "", None).unwrap();
        assert!(s.product_photo(p.id).unwrap().is_none());
        assert!(s.add_attachment("x", 1, "big.bin", &vec![0u8; MAX_ATTACHMENT + 1]).is_err());
    }

    #[test]
    fn products_loaded_from_supplier_invoice() {
        let s = Store::open_in_memory().unwrap();
        let v = s.vat_codes().unwrap().into_iter().find(|v| v.rate == dec!(22)).unwrap();
        let f = s.save_party(&Party { name: "Fornitore".into(), role: PartyRole::Supplier, vat_number: "09876543210".into(), ..Default::default() }).unwrap();
        let old = s.save_product(&Product { code: "OLD".into(), description: "Esistente".into(), supplier_code: "F-2".into(), supplier_id: Some(f.id), cost: dec!(1), ..Default::default() }).unwrap();
        let mut d = Document::new(DocKind::PurchaseInvoice);
        d.party_id = Some(f.id);
        d.external_number = "45/A".into();
        d.lines.push(DocLine { code: "F-1".into(), description: "Viti inox".into(), unit: "PZ".into(), qty: dec!(100), unit_price: dec!(0.20), discounts: "10".into(), vat: VatRef::from_code(&v), ..Default::default() });
        d.lines.push(DocLine { code: "F-2".into(), description: "Bulloni".into(), qty: dec!(10), unit_price: dec!(2), vat: VatRef::from_code(&v), ..Default::default() });
        d.lines.push(DocLine { description: "Spese di trasporto".into(), qty: dec!(1), unit_price: dec!(10), vat: VatRef::from_code(&v), ..Default::default() });
        let d = s.save_document(&d).unwrap();
        let d = s.issue_document(d.id).unwrap();
        let r = s.import_products_from_purchase(d.id, "", true).unwrap();
        assert_eq!((r.created, r.updated, r.skipped), (1, 1, 1));
        let new = s.product_by_code("F-1").unwrap().unwrap();
        assert_eq!(new.cost, dec!(0.18));
        assert_eq!(new.supplier_id, Some(f.id));
        assert_eq!(s.product(old.id).unwrap().unwrap().cost, dec!(2));
        // le righe sono collegate e caricano il magazzino
        assert_eq!(s.stock_of(new.id).unwrap(), dec!(100));
        assert_eq!(s.stock_of(old.id).unwrap(), dec!(10));
    }

    #[test]
    fn intent_plafond_usage() {
        let s = Store::open_in_memory().unwrap();
        let n35 = s.vat_codes().unwrap().into_iter().find(|v| v.natura.as_deref() == Some("N3.5")).expect("codice N3.5");
        let c = s
            .save_party(&Party {
                name: "Esportatore".into(),
                vat_number: "01234567890".into(),
                intents: vec![IntentDeclaration { protocol: "21012345678901234-000001".into(), year: 2026, limit: dec!(10000), ..Default::default() }],
                ..Default::default()
            })
            .unwrap();
        for amount in [dec!(3000), dec!(2500)] {
            let mut d = Document::new(DocKind::Invoice);
            d.party_id = Some(c.id);
            d.intent_protocol = "21012345678901234-000001".into();
            d.lines.push(DocLine { description: "Merce".into(), unit_price: amount, vat: VatRef::from_code(&n35), ..Default::default() });
            let d = s.save_document(&d).unwrap();
            s.issue_document(d.id).unwrap();
        }
        let u = s.intent_usage(c.id, "21012345678901234-000001", None).unwrap();
        assert_eq!(u.used, dec!(5500));
        assert_eq!(u.residual(), Some(dec!(4500)));
    }

    #[test]
    fn production_loads_finished_and_unloads_components() {
        let s = Store::open_in_memory().unwrap();
        let gamba = s.save_product(&Product { code: "GAMBA".into(), description: "Gamba".into(), cost: dec!(5), ..Default::default() }).unwrap();
        let piano = s.save_product(&Product { code: "PIANO".into(), description: "Piano".into(), cost: dec!(30), ..Default::default() }).unwrap();
        let tavolo = s
            .save_product(&Product {
                code: "TAV".into(),
                description: "Tavolo".into(),
                kind: ProductKind::Assembled,
                components: vec![Component { product_id: gamba.id, qty: dec!(4), note: String::new() }, Component { product_id: piano.id, qty: dec!(1), note: String::new() }],
                ..Default::default()
            })
            .unwrap();
        for (p, q) in [(gamba.id, dec!(10)), (piano.id, dec!(5))] {
            s.add_manual_stock_move(&StockMove { product_id: p, qty: q, reason: "carico".into(), ..Default::default() }).unwrap();
        }
        assert_eq!(s.bom_cost(&tavolo).unwrap(), dec!(50));
        assert_eq!(s.producible(&tavolo).unwrap(), Some(dec!(2)));
        let mut d = Document::new(DocKind::Production);
        d.lines.push(DocLine { product_id: Some(tavolo.id), description: "Tavolo".into(), qty: dec!(2), ..Default::default() });
        let d = s.save_document(&d).unwrap();
        s.issue_document(d.id).unwrap();
        assert_eq!(s.stock_of(tavolo.id).unwrap(), dec!(2));
        assert_eq!(s.stock_of(gamba.id).unwrap(), dec!(2));
        assert_eq!(s.stock_of(piano.id).unwrap(), dec!(3));
        let moves = s.stock_moves(Some(tavolo.id), 10).unwrap();
        assert_eq!(moves[0].unit_value, dec!(50));
    }
}
