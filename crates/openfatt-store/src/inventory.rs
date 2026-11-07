//! Inventario fisico: sessioni di conteggio per magazzino e data, con rettifiche automatiche.
//!
//! La sessione fotografa TUTTI gli articoli gestiti a magazzino, anche con giacenza zero o
//! negativa (una riga per variante, per lotto e per numero di serie). Alla chiusura, per ogni
//! riga contata si registra la differenza fra quantità contata e giacenza alla data
//! dell'inventario; le rettifiche sono collegate alla sessione e si annullano riaprendola.

use std::collections::BTreeMap;
use std::str::FromStr;

use chrono::NaiveDate;
use rusqlite::{params, OptionalExtension};
use rust_decimal::Decimal;

use openfatt_core::model::{StockMove, Tracking};

use crate::{date_from_sql, date_to_sql, dec_from_sql, dec_to_sql, now_sql, Result, Store, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryStatus {
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InventorySession {
    pub id: i64,
    pub number: i64,
    pub date: NaiveDate,
    pub warehouse: String,
    pub description: String,
    pub status: InventoryStatus,
    pub created_at: String,
    pub closed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct InventoryLine {
    pub id: i64,
    pub session_id: i64,
    pub product_id: i64,
    pub code: String,
    pub description: String,
    pub category: String,
    pub unit: String,
    pub barcode: String,
    pub cost: Decimal,
    pub variant: String,
    pub lot: String,
    pub expiry: Option<NaiveDate>,
    pub serial: String,
    /// Giacenza teorica alla data, al momento della creazione o dell'ultimo aggiornamento
    pub expected: Decimal,
    pub counted: Option<Decimal>,
    pub note: String,
}

impl InventoryLine {
    pub fn difference(&self) -> Option<Decimal> {
        self.counted.map(|c| c - self.expected)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct InventoryTotals {
    pub lines: usize,
    pub counted: usize,
    pub with_difference: usize,
    pub plus_value: Decimal,
    pub minus_value: Decimal,
    /// Valore al costo della merce contata (o teorica se non contata)
    pub stock_value: Decimal,
}

/// Esito dell'applicazione dei conteggi importati da file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CountImport {
    pub updated: usize,
    pub added: usize,
    pub errors: Vec<String>,
}

/// Esito della chiusura.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InventoryApplied {
    pub adjustments: usize,
    pub plus_qty: Decimal,
    pub minus_qty: Decimal,
}

fn status_code(s: InventoryStatus) -> &'static str {
    match s {
        InventoryStatus::Open => "APERTO",
        InventoryStatus::Closed => "CHIUSO",
    }
}

impl Store {
    /// Giacenza alla data (movimenti fino alla data compresa) per articolo/variante/lotto/seriale.
    pub fn stock_at(&self, product_id: i64, warehouse: &str, date: NaiveDate, variant: Option<&str>, lot: Option<&str>, serial: Option<&str>) -> Result<Decimal> {
        let mut st = self.conn().prepare(
            "SELECT qty FROM stock_moves WHERE product_id = ?1 AND warehouse = ?2 AND date <= ?3
             AND (?4 IS NULL OR variant = ?4) AND (?5 IS NULL OR lot = ?5) AND (?6 IS NULL OR serial = ?6)",
        )?;
        let rows = st.query_map(params![product_id, warehouse, date_to_sql(date), variant, lot, serial], |r| r.get::<_, String>(0))?;
        let mut t = Decimal::ZERO;
        for r in rows {
            t += dec_from_sql(Some(r?));
        }
        Ok(t)
    }

    /// Crea una sessione di inventario con tutte le righe da contare.
    pub fn create_inventory(&self, date: NaiveDate, warehouse: &str, description: &str, category: Option<&str>) -> Result<i64> {
        let wh = self.resolve_warehouse(warehouse)?;
        let open: Option<i64> = self
            .conn()
            .query_row("SELECT id FROM inventory_sessions WHERE warehouse = ?1 AND status = 'APERTO'", [&wh], |r| r.get(0))
            .optional()?;
        if open.is_some() {
            return Err(StoreError::Invalid(format!("Esiste già un inventario aperto per il magazzino {}: chiuderlo o eliminarlo prima", wh)));
        }
        let tx = self.begin()?;
        let number = self.next_counter(&format!("inventory_{}", chrono::Datelike::year(&date)))? as i64;
        self.conn().execute(
            "INSERT INTO inventory_sessions(number, date, warehouse, description, status, created_at) VALUES (?1, ?2, ?3, ?4, 'APERTO', ?5)",
            params![number, date_to_sql(date), wh, description.trim(), now_sql()],
        )?;
        let sid = self.conn().last_insert_rowid();
        let mut pos = 0i64;
        let products = self.products("", false)?;
        for p in products.into_iter().filter(|p| p.manage_stock && !p.is_service) {
            if let Some(c) = category {
                if !p.category.trim().eq_ignore_ascii_case(c.trim()) {
                    continue;
                }
            }
            // chiavi (variante, lotto, scadenza, seriale) con la loro giacenza alla data
            let mut keys: BTreeMap<(String, String, String), (Option<NaiveDate>, Decimal)> = BTreeMap::new();
            let mut st = self.conn().prepare("SELECT variant, lot, serial, expiry, qty FROM stock_moves WHERE product_id = ?1 AND warehouse = ?2 AND date <= ?3")?;
            let rows = st.query_map(params![p.id, wh, date_to_sql(date)], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?))
            })?;
            for r in rows {
                let (v, l, s, e, q) = r?;
                let (l, s) = match p.tracking {
                    Tracking::None => (String::new(), String::new()),
                    Tracking::Lot => (l, String::new()),
                    Tracking::Serial => (String::new(), s),
                };
                let v = if p.variants.is_empty() { String::new() } else { v };
                let entry = keys.entry((v, l, s)).or_insert((None, Decimal::ZERO));
                if let Some(x) = e.map(|x| date_from_sql(&x)) {
                    entry.0 = Some(entry.0.map(|c| c.min(x)).unwrap_or(x));
                }
                entry.1 += dec_from_sql(Some(q));
            }
            // lotti e seriali esauriti non servono; le righe a zero dell'articolo invece sì
            keys.retain(|(_, l, s), (_, q)| (l.is_empty() && s.is_empty()) || !q.is_zero());
            // ogni variante va contata, anche se mai movimentata
            for c in p.variants.combinations() {
                if !keys.keys().any(|(v, _, _)| v == &c) {
                    keys.insert((c, String::new(), String::new()), (None, Decimal::ZERO));
                }
            }
            if p.variants.is_empty() && !keys.keys().any(|(v, l, s)| v.is_empty() && (l.is_empty() || p.tracking == Tracking::Lot) && (s.is_empty() || p.tracking == Tracking::Serial)) {
                keys.insert((String::new(), String::new(), String::new()), (None, Decimal::ZERO));
            }
            if !p.variants.is_empty() {
                keys.retain(|(v, _, _), (_, q)| !v.is_empty() || !q.is_zero());
            }
            for ((v, l, s), (e, q)) in keys {
                pos += 1;
                self.conn().execute(
                    "INSERT INTO inventory_lines(session_id, position, product_id, variant, lot, expiry, serial, expected) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![sid, pos, p.id, v, l, e.map(date_to_sql), s, dec_to_sql(q)],
                )?;
            }
        }
        self.audit("inventory", sid, "create", &format!("n. {} magazzino {}", number, wh))?;
        tx.commit()?;
        Ok(sid)
    }

    pub fn inventory_sessions(&self) -> Result<Vec<InventorySession>> {
        let mut st = self.conn().prepare("SELECT id, number, date, warehouse, description, status, created_at, closed_at FROM inventory_sessions ORDER BY date DESC, id DESC")?;
        let rows = st.query_map([], |r| {
            Ok(InventorySession {
                id: r.get(0)?,
                number: r.get(1)?,
                date: date_from_sql(&r.get::<_, String>(2)?),
                warehouse: r.get(3)?,
                description: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                status: if r.get::<_, String>(5)? == "CHIUSO" { InventoryStatus::Closed } else { InventoryStatus::Open },
                created_at: r.get(6)?,
                closed_at: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn inventory_session(&self, id: i64) -> Result<InventorySession> {
        self.inventory_sessions()?.into_iter().find(|s| s.id == id).ok_or_else(|| StoreError::NotFound("inventario".into()))
    }

    pub fn inventory_lines(&self, session_id: i64) -> Result<Vec<InventoryLine>> {
        let mut st = self.conn().prepare(
            "SELECT l.id, l.product_id, l.variant, l.lot, l.expiry, l.serial, l.expected, l.counted, l.note, p.data
             FROM inventory_lines l JOIN products p ON p.id = l.product_id WHERE l.session_id = ?1 ORDER BY l.position, l.id",
        )?;
        let rows = st.query_map([session_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, String>(9)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, pid, v, l, e, s, exp, cnt, note, pdata) = r?;
            let p: openfatt_core::model::Product = serde_json::from_str(&pdata)?;
            out.push(InventoryLine {
                id,
                session_id,
                product_id: pid,
                code: p.code.clone(),
                description: p.description.clone(),
                category: p.category.clone(),
                unit: p.unit.clone(),
                barcode: if v.is_empty() { p.barcode.clone() } else { p.variants.barcode_for(&v).unwrap_or("").to_string() },
                cost: p.cost,
                variant: v,
                lot: l,
                expiry: e.map(|x| date_from_sql(&x)),
                serial: s,
                expected: dec_from_sql(Some(exp)),
                counted: cnt.and_then(|c| Decimal::from_str(&c).ok()),
                note: note.unwrap_or_default(),
            });
        }
        Ok(out)
    }

    fn require_open(&self, session_id: i64) -> Result<InventorySession> {
        let s = self.inventory_session(session_id)?;
        if s.status != InventoryStatus::Open {
            return Err(StoreError::Invalid("Inventario chiuso: riaprirlo per modificarlo".into()));
        }
        Ok(s)
    }

    /// Imposta la quantità contata (None = non contato) e la nota di una riga.
    pub fn set_inventory_count(&self, line_id: i64, counted: Option<Decimal>, note: Option<&str>) -> Result<()> {
        let sid: i64 = self.conn().query_row("SELECT session_id FROM inventory_lines WHERE id = ?1", [line_id], |r| r.get(0))?;
        self.require_open(sid)?;
        if let Some(c) = counted {
            if c.is_sign_negative() {
                return Err(StoreError::Invalid("La quantità contata non può essere negativa".into()));
            }
        }
        self.conn().execute("UPDATE inventory_lines SET counted = ?1 WHERE id = ?2", params![counted.map(dec_to_sql), line_id])?;
        if let Some(n) = note {
            self.conn().execute("UPDATE inventory_lines SET note = ?1 WHERE id = ?2", params![n, line_id])?;
        }
        Ok(())
    }

    /// Aggiunge una riga (lotto o seriale trovato ma non registrato, variante nuova...).
    pub fn add_inventory_line(&self, session_id: i64, product_id: i64, variant: &str, lot: &str, expiry: Option<NaiveDate>, serial: &str, counted: Option<Decimal>) -> Result<i64> {
        let s = self.require_open(session_id)?;
        let p = self.product(product_id)?.ok_or_else(|| StoreError::NotFound("articolo".into()))?;
        if !p.variants.is_empty() && !p.variants.contains(variant.trim()) {
            return Err(StoreError::Invalid(format!("Indicare una variante valida di {}", p.code)));
        }
        if p.tracking == Tracking::Serial && serial.trim().is_empty() {
            return Err(StoreError::Invalid(format!("{} è gestito a numeri di serie: indicare il seriale", p.code)));
        }
        let lines = self.inventory_lines(session_id)?;
        if lines.iter().any(|l| l.product_id == product_id && l.variant == variant.trim() && l.lot == lot.trim() && l.serial == serial.trim()) {
            return Err(StoreError::Invalid("La riga è già presente nell'inventario".into()));
        }
        let expected = self.stock_at(product_id, &s.warehouse, s.date, Some(variant.trim()), Some(lot.trim()), Some(serial.trim()))?;
        let pos: i64 = self.conn().query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM inventory_lines WHERE session_id = ?1", [session_id], |r| r.get(0))?;
        self.conn().execute(
            "INSERT INTO inventory_lines(session_id, position, product_id, variant, lot, expiry, serial, expected, counted) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![session_id, pos, product_id, variant.trim(), lot.trim(), expiry.map(date_to_sql), serial.trim(), dec_to_sql(expected), counted.map(dec_to_sql)],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    /// Lettura di un codice a barre durante il conteggio: aggiunge `qty` alla riga corrispondente.
    /// Restituisce la riga aggiornata.
    pub fn inventory_scan(&self, session_id: i64, code: &str, qty: Decimal) -> Result<InventoryLine> {
        self.require_open(session_id)?;
        // il codice letto può essere il numero di serie di un'unità: conta 1
        if let Some(l) = self.inventory_lines(session_id)?.into_iter().find(|l| !l.serial.is_empty() && l.serial.eq_ignore_ascii_case(code.trim())) {
            self.set_inventory_count(l.id, Some(Decimal::ONE), None)?;
            return Ok(InventoryLine { counted: Some(Decimal::ONE), ..l });
        }
        let (p, variant) = self.find_by_code(code)?.ok_or_else(|| StoreError::NotFound(format!("articolo con codice '{}'", code.trim())))?;
        if p.tracking == Tracking::Serial {
            // per i seriali il codice letto può essere il seriale stesso
            return Err(StoreError::Invalid(format!("{} è gestito a numeri di serie: leggere o indicare il seriale nella riga", p.code)));
        }
        let lines = self.inventory_lines(session_id)?;
        let candidates: Vec<&InventoryLine> = lines.iter().filter(|l| l.product_id == p.id && (variant.is_empty() || l.variant == variant)).collect();
        let line = match candidates.as_slice() {
            [] => {
                let id = self.add_inventory_line(session_id, p.id, &variant, "", None, "", None)?;
                self.inventory_lines(session_id)?.into_iter().find(|l| l.id == id).unwrap()
            }
            [one] => (*one).clone(),
            many => {
                // più righe (lotti o varianti): si usa la prima ancora da completare
                many.iter().find(|l| l.counted.map(|c| c < l.expected).unwrap_or(true)).copied().unwrap_or(many[0]).clone()
            }
        };
        let new = line.counted.unwrap_or_default() + qty;
        self.set_inventory_count(line.id, Some(new.max(Decimal::ZERO)), None)?;
        Ok(InventoryLine { counted: Some(new.max(Decimal::ZERO)), ..line })
    }

    /// Applica i conteggi letti da un file. Le righe si ritrovano per ID, altrimenti per
    /// codice + variante + lotto + seriale; le combinazioni nuove di articoli esistenti si aggiungono.
    pub fn import_inventory_counts(&self, session_id: i64, rows: &[ga_core_count::CountRow]) -> Result<CountImport> {
        self.require_open(session_id)?;
        let lines = self.inventory_lines(session_id)?;
        let mut res = CountImport::default();
        let tx = self.begin()?;
        for r in rows {
            let target = match r.line_id {
                Some(id) => match lines.iter().find(|l| l.id == id) {
                    Some(l) => Some(l.clone()),
                    None => {
                        res.errors.push(format!("Riga {}: ID {} non appartiene a questo inventario", r.file_row, id));
                        continue;
                    }
                },
                None => lines
                    .iter()
                    .find(|l| l.code.eq_ignore_ascii_case(r.code.trim()) && l.variant == r.variant.trim() && l.lot == r.lot.trim() && l.serial == r.serial.trim())
                    .cloned(),
            };
            match target {
                Some(l) => {
                    if l.counted != r.counted || (!r.note.is_empty() && r.note != l.note) {
                        self.set_inventory_count(l.id, r.counted, if r.note.is_empty() { None } else { Some(&r.note) })?;
                        res.updated += 1;
                    }
                }
                None => {
                    if r.counted.is_none() {
                        continue;
                    }
                    match self.find_by_code(r.code.trim())? {
                        Some((p, v)) => {
                            let variant = if r.variant.trim().is_empty() { v } else { r.variant.trim().to_string() };
                            let expiry = NaiveDate::parse_from_str(r.expiry.trim(), "%d/%m/%Y").or_else(|_| NaiveDate::parse_from_str(r.expiry.trim(), "%Y-%m-%d")).ok();
                            match self.add_inventory_line(session_id, p.id, &variant, &r.lot, expiry, &r.serial, r.counted) {
                                Ok(_) => res.added += 1,
                                Err(e) => res.errors.push(format!("Riga {}: {}", r.file_row, e)),
                            }
                        }
                        None => res.errors.push(format!("Riga {}: articolo '{}' inesistente", r.file_row, r.code)),
                    }
                }
            }
        }
        self.audit("inventory", session_id, "import", &format!("{} aggiornate, {} aggiunte, {} errori", res.updated, res.added, res.errors.len()))?;
        tx.commit()?;
        Ok(res)
    }

    pub fn inventory_totals(&self, session_id: i64) -> Result<InventoryTotals> {
        let mut t = InventoryTotals::default();
        for l in self.inventory_lines(session_id)? {
            t.lines += 1;
            if let Some(c) = l.counted {
                t.counted += 1;
                let d = c - l.expected;
                if !d.is_zero() {
                    t.with_difference += 1;
                    if d > Decimal::ZERO {
                        t.plus_value += d * l.cost;
                    } else {
                        t.minus_value += -d * l.cost;
                    }
                }
            }
            t.stock_value += l.counted.unwrap_or(l.expected) * l.cost;
        }
        Ok(t)
    }

    /// Chiude l'inventario registrando le rettifiche. Le righe non contate restano invariate,
    /// oppure si considerano a zero se `uncounted_as_zero`.
    pub fn close_inventory(&self, session_id: i64, uncounted_as_zero: bool) -> Result<InventoryApplied> {
        let s = self.require_open(session_id)?;
        let lines = self.inventory_lines(session_id)?;
        let mut out = InventoryApplied::default();
        let tx = self.begin()?;
        let reason = format!("Inventario n. {} del {}", s.number, s.date.format("%d/%m/%Y"));
        for l in &lines {
            let counted = match (l.counted, uncounted_as_zero) {
                (Some(c), _) => c,
                (None, true) => Decimal::ZERO,
                (None, false) => continue,
            };
            // differenza rispetto alla giacenza alla data, ricalcolata ora
            let current = self.stock_at(l.product_id, &s.warehouse, s.date, Some(&l.variant), Some(&l.lot), Some(&l.serial))?;
            let diff = counted - current;
            if diff.is_zero() {
                continue;
            }
            let product = self.product(l.product_id)?.ok_or_else(|| StoreError::NotFound("articolo".into()))?;
            let base = StockMove {
                id: 0,
                date: s.date,
                product_id: l.product_id,
                qty: diff,
                warehouse: s.warehouse.clone(),
                unit_value: product.cost,
                reason: reason.clone(),
                document_id: None,
                line_id: None,
                lot: if product.tracking == Tracking::Lot && l.lot.is_empty() && diff > Decimal::ZERO { crate::LOT_UNKNOWN.into() } else { l.lot.clone() },
                variant: l.variant.clone(),
                expiry: l.expiry,
                serial: String::new(),
            };
            let serials: Vec<String> = if product.tracking == Tracking::Serial && !l.serial.is_empty() { vec![l.serial.clone(); 1] } else { vec![] };
            if product.tracking == Tracking::Serial && (l.serial.is_empty() || diff.abs() != Decimal::ONE) {
                if diff.is_zero() {
                    continue;
                }
                return Err(StoreError::Invalid(format!("{}: per i numeri di serie contare 0 o 1 su ogni riga con il seriale", product.code)));
            }
            let moves = self.post_tracked_move(&product, base, &serials, 0).map_err(|e| StoreError::Invalid(format!("{} {}: {}", product.code, l.variant, e)))?;
            for m in &moves {
                self.conn().execute("UPDATE stock_moves SET inventory_id = ?1 WHERE id = ?2", params![session_id, m.id])?;
            }
            out.adjustments += 1;
            if diff > Decimal::ZERO {
                out.plus_qty += diff;
            } else {
                out.minus_qty += -diff;
            }
        }
        self.conn().execute("UPDATE inventory_sessions SET status = 'CHIUSO', closed_at = ?1 WHERE id = ?2", params![now_sql(), session_id])?;
        self.audit("inventory", session_id, "close", &format!("{} rettifiche", out.adjustments))?;
        tx.commit()?;
        Ok(out)
    }

    /// Riapre un inventario chiuso eliminando le sue rettifiche.
    pub fn reopen_inventory(&self, session_id: i64) -> Result<usize> {
        let s = self.inventory_session(session_id)?;
        if s.status == InventoryStatus::Open {
            return Ok(0);
        }
        let other_open: Option<i64> = self
            .conn()
            .query_row("SELECT id FROM inventory_sessions WHERE warehouse = ?1 AND status = 'APERTO' AND id <> ?2", params![s.warehouse, session_id], |r| r.get(0))
            .optional()?;
        if other_open.is_some() {
            return Err(StoreError::Invalid("C'è già un altro inventario aperto per questo magazzino".into()));
        }
        let tx = self.begin()?;
        let n = self.conn().execute("DELETE FROM stock_moves WHERE inventory_id = ?1", [session_id])?;
        self.conn().execute("UPDATE inventory_sessions SET status = ?1, closed_at = NULL WHERE id = ?2", params![status_code(InventoryStatus::Open), session_id])?;
        self.audit("inventory", session_id, "reopen", &format!("{} rettifiche annullate", n))?;
        tx.commit()?;
        Ok(n)
    }

    pub fn delete_inventory(&self, session_id: i64) -> Result<()> {
        self.require_open(session_id)?;
        self.conn().execute("DELETE FROM inventory_sessions WHERE id = ?1", [session_id])?;
        self.audit("inventory", session_id, "delete", "")?;
        Ok(())
    }

    /// Aggiorna la giacenza teorica delle righe (se ci sono stati movimenti con data anteriore).
    pub fn refresh_inventory_expected(&self, session_id: i64) -> Result<usize> {
        let s = self.require_open(session_id)?;
        let mut changed = 0;
        for l in self.inventory_lines(session_id)? {
            let v = self.stock_at(l.product_id, &s.warehouse, s.date, Some(&l.variant), Some(&l.lot), Some(&l.serial))?;
            if v != l.expected {
                self.conn().execute("UPDATE inventory_lines SET expected = ?1 WHERE id = ?2", params![dec_to_sql(v), l.id])?;
                changed += 1;
            }
        }
        Ok(changed)
    }
}

/// Tipo di riga importata (riesportato per non legare l'archivio al formato dei file).
pub mod ga_core_count {
    use rust_decimal::Decimal;
    #[derive(Debug, Clone, PartialEq, Default)]
    pub struct CountRow {
        pub file_row: usize,
        pub line_id: Option<i64>,
        pub code: String,
        pub variant: String,
        pub lot: String,
        pub expiry: String,
        pub serial: String,
        pub counted: Option<Decimal>,
        pub note: String,
    }
}

#[cfg(test)]
mod tests {
    use super::ga_core_count::CountRow;
    use super::*;
    use openfatt_core::dec;
    use openfatt_core::model::*;

    fn d(dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 12, dd).unwrap()
    }

    #[test]
    fn inventory_includes_zero_stock_and_applies_adjustments() {
        let s = Store::open_in_memory().unwrap();
        let a = s.save_product(&Product { code: "A".into(), description: "Con giacenza".into(), cost: dec!(2), ..Default::default() }).unwrap();
        let z = s.save_product(&Product { code: "Z".into(), description: "Mai movimentato".into(), cost: dec!(5), ..Default::default() }).unwrap();
        let n = s.save_product(&Product { code: "N".into(), description: "Negativo".into(), ..Default::default() }).unwrap();
        s.save_product(&Product { code: "S".into(), description: "Servizio".into(), is_service: true, ..Default::default() }).unwrap();
        s.add_manual_stock_move(&StockMove { product_id: a.id, qty: dec!(10), date: d(1), ..Default::default() }).unwrap();
        s.add_manual_stock_move(&StockMove { product_id: n.id, qty: dec!(-3), date: d(1), ..Default::default() }).unwrap();
        // movimento successivo alla data dell'inventario: non conta
        s.add_manual_stock_move(&StockMove { product_id: a.id, qty: dec!(100), date: d(20), ..Default::default() }).unwrap();

        let sid = s.create_inventory(d(15), "", "Fine anno", None).unwrap();
        let lines = s.inventory_lines(sid).unwrap();
        // tutti gli articoli gestiti, anche a zero e negativi; esclusi i servizi
        let codes: Vec<&str> = lines.iter().map(|l| l.code.as_str()).collect();
        assert_eq!(codes, vec!["A", "N", "Z"]);
        assert_eq!(lines.iter().find(|l| l.code == "A").unwrap().expected, dec!(10));
        assert_eq!(lines.iter().find(|l| l.code == "Z").unwrap().expected, dec!(0));
        assert_eq!(lines.iter().find(|l| l.code == "N").unwrap().expected, dec!(-3));
        // un secondo inventario aperto sullo stesso magazzino: rifiutato
        assert!(s.create_inventory(d(15), "", "", None).is_err());

        // conteggi: A 8 (per ID), Z 4 (per codice, articolo che risultava a zero), N non contato
        let la = lines.iter().find(|l| l.code == "A").unwrap();
        let res = s
            .import_inventory_counts(
                sid,
                &[
                    CountRow { file_row: 2, line_id: Some(la.id), counted: Some(dec!(8)), ..Default::default() },
                    CountRow { file_row: 3, code: "z".into(), counted: Some(dec!(4)), note: "trovati in retro".into(), ..Default::default() },
                    CountRow { file_row: 4, code: "XXX".into(), counted: Some(dec!(1)), ..Default::default() },
                ],
            )
            .unwrap();
        assert_eq!((res.updated, res.added, res.errors.len()), (2, 0, 1));
        let t = s.inventory_totals(sid).unwrap();
        assert_eq!((t.counted, t.with_difference), (2, 2));
        assert_eq!(t.minus_value, dec!(4)); // 2 pezzi x 2
        assert_eq!(t.plus_value, dec!(20)); // 4 pezzi x 5

        let applied = s.close_inventory(sid, false).unwrap();
        assert_eq!(applied.adjustments, 2);
        assert_eq!(s.stock_at(a.id, DEFAULT_WAREHOUSE_NAME, d(15), None, None, None).unwrap(), dec!(8));
        assert_eq!(s.stock_of(z.id).unwrap(), dec!(4));
        assert_eq!(s.stock_of(n.id).unwrap(), dec!(-3)); // non contato: invariato
        assert_eq!(s.stock_of(a.id).unwrap(), dec!(108)); // il carico successivo resta
        assert!(s.set_inventory_count(la.id, Some(dec!(1)), None).is_err());
        // riapertura: rettifiche annullate
        assert_eq!(s.reopen_inventory(sid).unwrap(), 2);
        assert_eq!(s.stock_of(z.id).unwrap(), dec!(0));
        // chiusura con non contati a zero: N passa da -3 a 0
        s.close_inventory(sid, true).unwrap();
        assert_eq!(s.stock_of(n.id).unwrap(), dec!(0));
    }

    const DEFAULT_WAREHOUSE_NAME: &str = crate::DEFAULT_WAREHOUSE;

    #[test]
    fn variants_lots_and_scanning() {
        let s = Store::open_in_memory().unwrap();
        let m = s
            .save_product(&Product {
                code: "M".into(),
                description: "Maglia".into(),
                variants: VariantSpec { values1: vec!["S".into(), "M".into()], barcodes: vec![VariantBarcode { variant: "M".into(), barcode: "2000000000015".into() }], ..Default::default() },
                ..Default::default()
            })
            .unwrap();
        let l = s.save_product(&Product { code: "L".into(), description: "Colla".into(), tracking: Tracking::Lot, ..Default::default() }).unwrap();
        s.add_manual_stock_move(&StockMove { product_id: m.id, qty: dec!(2), variant: "S".into(), date: d(1), ..Default::default() }).unwrap();
        s.add_manual_stock_move(&StockMove { product_id: l.id, qty: dec!(5), lot: "L1".into(), date: d(1), ..Default::default() }).unwrap();
        let sid = s.create_inventory(d(10), "", "", None).unwrap();
        let lines = s.inventory_lines(sid).unwrap();
        // una riga per ogni variante (anche M mai movimentata) e una per il lotto
        assert_eq!(lines.iter().filter(|x| x.code == "M").map(|x| x.variant.as_str()).collect::<Vec<_>>(), vec!["M", "S"]);
        assert!(lines.iter().any(|x| x.code == "L" && x.lot == "L1" && x.expected == dec!(5)));
        // lettura del codice a barre della variante M due volte
        s.inventory_scan(sid, "2000000000015", Decimal::ONE).unwrap();
        let r = s.inventory_scan(sid, "2000000000015", Decimal::ONE).unwrap();
        assert_eq!((r.variant.as_str(), r.counted), ("M", Some(dec!(2))));
        // lotto trovato ma non registrato: riga aggiunta
        s.import_inventory_counts(sid, &[CountRow { code: "L".into(), lot: "L9".into(), counted: Some(dec!(3)), ..Default::default() }]).unwrap();
        let ll = s.inventory_lines(sid).unwrap().into_iter().find(|x| x.code == "L" && x.lot == "L1").unwrap();
        s.set_inventory_count(ll.id, Some(dec!(4)), None).unwrap();
        s.close_inventory(sid, false).unwrap();
        assert_eq!(s.stock_by_variant(m.id).unwrap(), vec![("M".to_string(), dec!(2)), ("S".to_string(), dec!(2))]);
        let lots = s.lots_available(l.id, None, None).unwrap();
        assert_eq!(lots.iter().map(|x| (x.lot.as_str(), x.qty)).collect::<Vec<_>>(), vec![("L1", dec!(4)), ("L9", dec!(3))]);
    }
}
