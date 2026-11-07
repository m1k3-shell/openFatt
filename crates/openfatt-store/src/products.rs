use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};
use rust_decimal::Decimal;

use openfatt_core::model::{Product, StockMove, Tracking};

use crate::{date_from_sql, date_to_sql, dec_from_sql, dec_to_sql, now_sql, Result, Store, StoreError};

#[derive(Debug, Clone)]
pub struct StockRow {
    pub product: Product,
    pub stock: Decimal,
    /// Quantità impegnata da ordini clienti emessi e non ancora evasi.
    pub ordered_by_customers: Decimal,
    /// Quantità in arrivo da ordini fornitori emessi e non ancora ricevuti.
    pub incoming: Decimal,
    pub value: Decimal,
    pub under_min: bool,
}

impl Store {
    pub fn products(&self, search: &str, include_inactive: bool) -> Result<Vec<Product>> {
        self.products_page(search, include_inactive, usize::MAX)
    }

    /// Come `products`, ma al massimo `limit` articoli: per le ricerche mentre si scrive.
    /// Con molti articoli i codici che iniziano con il testo cercato vengono prima.
    pub fn products_page(&self, search: &str, include_inactive: bool, limit: usize) -> Result<Vec<Product>> {
        let like = format!("%{}%", search.trim());
        let prefix = format!("{}%", search.trim());
        let limit = limit.min(i64::MAX as usize) as i64;
        let mut st = self.conn().prepare_cached(
            "SELECT id, version, data FROM products
             WHERE (?1 = '%%' OR code LIKE ?1 OR description LIKE ?1 OR barcode = ?2 OR category LIKE ?1)
               AND (?3 = 1 OR active = 1)
             ORDER BY CASE WHEN ?1 <> '%%' AND code LIKE ?5 THEN 0 ELSE 1 END, code COLLATE NOCASE
             LIMIT ?4",
        )?;
        let rows = st.query_map(params![like, search.trim(), include_inactive as i64, limit, prefix], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, version, data) = row?;
            let mut p: Product = serde_json::from_str(&data)?;
            p.id = id;
            p.version = version;
            out.push(p);
        }
        Ok(out)
    }

    pub fn product(&self, id: i64) -> Result<Option<Product>> {
        let row: Option<(i64, String)> = self
            .conn()
            .query_row("SELECT version, data FROM products WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        Ok(match row {
            None => None,
            Some((v, d)) => {
                let mut p: Product = serde_json::from_str(&d)?;
                p.id = id;
                p.version = v;
                Some(p)
            }
        })
    }

    /// Ricerca per codice, codice a barre o codice a barre di variante.
    /// Restituisce l'articolo e la variante riconosciuta (vuota se generico).
    pub fn find_by_code(&self, code: &str) -> Result<Option<(Product, String)>> {
        if let Some(p) = self.product_by_code(code)? {
            return Ok(Some((p, String::new())));
        }
        let row: Option<(i64, String)> = self
            .conn()
            .query_row("SELECT product_id, variant FROM product_barcodes WHERE barcode = ?1", [code.trim()], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        match row {
            Some((id, variant)) => Ok(self.product(id)?.map(|p| (p, variant))),
            None => Ok(None),
        }
    }

    /// Ricerca esatta per codice o codice a barre (lettore barcode).
    pub fn product_by_code(&self, code: &str) -> Result<Option<Product>> {
        let id: Option<i64> = self
            .conn()
            .query_row(
                "SELECT id FROM products WHERE code = ?1 OR (barcode <> '' AND barcode = ?1) LIMIT 1",
                [code.trim()],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.product(id),
            None => Ok(None),
        }
    }

    /// Modifica più articoli insieme: tutte le modifiche o nessuna.
    pub fn update_products(&self, ids: &[i64], f: impl Fn(&mut Product)) -> Result<usize> {
        let tx = self.begin()?;
        let mut n = 0;
        for id in ids {
            if let Some(mut p) = self.product(*id)? {
                f(&mut p);
                self.save_product(&p)?;
                n += 1;
            }
        }
        self.audit("product", 0, "bulk_update", &format!("{} articoli", n))?;
        tx.commit()?;
        Ok(n)
    }

    pub fn save_product(&self, p: &Product) -> Result<Product> {
        let mut p = p.clone();
        p.code = p.code.trim().to_string();
        if p.code.is_empty() {
            p.code = format!("ART{:05}", self.next_counter("product_code")?);
        }
        if p.description.trim().is_empty() {
            return Err(StoreError::Invalid("La descrizione dell'articolo è obbligatoria".into()));
        }
        if p.is_service {
            p.manage_stock = false;
            p.tracking = Tracking::None;
        }
        // codici a barre delle varianti: univoci e riferiti a varianti esistenti
        let combos = p.variants.combinations();
        p.variants.barcodes.retain(|b| !b.barcode.trim().is_empty() && combos.contains(&b.variant));
        for b in &p.variants.barcodes {
            let other: Option<i64> = self
                .conn()
                .query_row(
                    "SELECT product_id FROM product_barcodes WHERE barcode = ?1 AND product_id <> ?2
                     UNION SELECT id FROM products WHERE barcode = ?1 AND id <> ?2 LIMIT 1",
                    params![b.barcode.trim(), p.id],
                    |r| r.get(0),
                )
                .optional()?;
            if other.is_some() {
                return Err(StoreError::Invalid(format!("Il codice a barre {} è già usato da un altro articolo", b.barcode)));
            }
        }
        let data = serde_json::to_string(&p)?;
        if p.id == 0 {
            self.conn()
                .execute(
                    "INSERT INTO products(code, description, barcode, category, active, version, data, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)",
                    params![p.code, p.description, p.barcode, p.category, p.active as i64, data, now_sql()],
                )
                .map_err(|e| match e {
                    rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
                        StoreError::Invalid(format!("Esiste già un articolo con codice '{}'", p.code))
                    }
                    other => StoreError::Db(other),
                })?;
            p.id = self.conn().last_insert_rowid();
            p.version = 1;
            self.write_variant_barcodes(&p)?;
            self.audit("product", p.id, "insert", &p.code)?;
        } else {
            let n = self.conn().execute(
                "UPDATE products SET code=?1, description=?2, barcode=?3, category=?4, active=?5, version=version+1, data=?6, updated_at=?7
                 WHERE id=?8 AND version=?9",
                params![p.code, p.description, p.barcode, p.category, p.active as i64, data, now_sql(), p.id, p.version],
            )?;
            if n == 0 {
                return Err(StoreError::Conflict(format!("L'articolo '{}' è stato modificato altrove: ricaricare", p.code)));
            }
            p.version += 1;
            self.write_variant_barcodes(&p)?;
            self.audit("product", p.id, "update", &p.code)?;
        }
        Ok(p)
    }

    pub fn delete_product(&self, id: i64) -> Result<()> {
        let used: i64 = self.conn().query_row(
            "SELECT (SELECT COUNT(*) FROM doc_lines WHERE product_id = ?1) + (SELECT COUNT(*) FROM stock_moves WHERE product_id = ?1)",
            [id],
            |r| r.get(0),
        )?;
        if used > 0 {
            return Err(StoreError::Invalid("L'articolo ha movimenti o righe di documento: si può solo disattivare".into()));
        }
        self.conn().execute("DELETE FROM products WHERE id = ?1", [id])?;
        self.audit("product", id, "delete", "")?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Magazzino
    // ------------------------------------------------------------------

    pub fn stock_of(&self, product_id: i64) -> Result<Decimal> {
        let mut st = self.conn().prepare("SELECT qty FROM stock_moves WHERE product_id = ?1")?;
        let rows = st.query_map([product_id], |r| r.get::<_, String>(0))?;
        let mut total = Decimal::ZERO;
        for r in rows {
            total += dec_from_sql(Some(r?));
        }
        Ok(total)
    }

    /// Giacenza di tutti gli articoli (somma dei movimenti), in una sola lettura.
    pub fn stock_levels(&self) -> Result<HashMap<i64, Decimal>> {
        self.stock_map()
    }

    fn stock_map(&self) -> Result<HashMap<i64, Decimal>> {
        let mut st = self.conn().prepare("SELECT product_id, qty FROM stock_moves")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut m: HashMap<i64, Decimal> = HashMap::new();
        for r in rows {
            let (id, q) = r?;
            *m.entry(id).or_default() += dec_from_sql(Some(q));
        }
        Ok(m)
    }

    /// Quantità residue da evadere degli ordini emessi, per articolo.
    fn open_orders(&self, kind: &str) -> Result<HashMap<i64, Decimal>> {
        let mut st = self.conn().prepare(
            "SELECT l.id, l.product_id, l.qty FROM doc_lines l JOIN documents d ON d.id = l.document_id
             WHERE d.kind = ?1 AND d.status = 'EMESSO' AND l.product_id IS NOT NULL",
        )?;
        let rows = st.query_map([kind], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))?;
        let mut m: HashMap<i64, Decimal> = HashMap::new();
        for r in rows {
            let (line_id, pid, q) = r?;
            let residual = dec_from_sql(Some(q)) - self.fulfilled_qty(line_id)?;
            if residual > Decimal::ZERO {
                *m.entry(pid).or_default() += residual;
            }
        }
        Ok(m)
    }

    pub fn stock_report(&self, search: &str, warehouse: Option<&str>) -> Result<Vec<StockRow>> {
        let stock = match warehouse {
            None => self.stock_map()?,
            Some(w) => {
                let mut m = HashMap::new();
                let mut st = self.conn().prepare("SELECT product_id, qty FROM stock_moves WHERE warehouse = ?1")?;
                let rows = st.query_map([w], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
                for r in rows {
                    let (id, q) = r?;
                    *m.entry(id).or_insert(Decimal::ZERO) += dec_from_sql(Some(q));
                }
                m
            }
        };
        let ordered = self.open_orders("ORC")?;
        let incoming = self.open_orders("ORF")?;
        let mut out = Vec::new();
        for p in self.products(search, false)? {
            if !p.manage_stock {
                continue;
            }
            let s = stock.get(&p.id).copied().unwrap_or_default();
            out.push(StockRow {
                value: s * p.cost,
                under_min: !p.min_stock.is_zero() && s < p.min_stock,
                ordered_by_customers: ordered.get(&p.id).copied().unwrap_or_default(),
                incoming: incoming.get(&p.id).copied().unwrap_or_default(),
                stock: s,
                product: p,
            });
        }
        Ok(out)
    }

    pub fn stock_moves(&self, product_id: Option<i64>, limit: usize) -> Result<Vec<StockMove>> {
        let mut st = self.conn().prepare(
            "SELECT id, date, product_id, qty, warehouse, unit_value, reason, document_id, line_id, lot, variant, expiry, serial FROM stock_moves
             WHERE (?1 IS NULL OR product_id = ?1) ORDER BY date DESC, id DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![product_id, limit as i64], |r| {
            Ok(StockMove {
                id: r.get(0)?,
                date: date_from_sql(&r.get::<_, String>(1)?),
                product_id: r.get(2)?,
                qty: dec_from_sql(r.get(3)?),
                warehouse: r.get(4)?,
                unit_value: dec_from_sql(r.get(5)?),
                reason: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                document_id: r.get(7)?,
                line_id: r.get(8)?,
                lot: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
                variant: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
                expiry: r.get::<_, Option<String>>(11)?.map(|s| date_from_sql(&s)),
                serial: r.get::<_, Option<String>>(12)?.unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub(crate) fn insert_stock_move(&self, m: &StockMove) -> Result<i64> {
        self.conn().execute(
            "INSERT INTO stock_moves(date, product_id, qty, warehouse, unit_value, reason, document_id, line_id, lot, variant, expiry, serial)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                date_to_sql(m.date),
                m.product_id,
                dec_to_sql(m.qty),
                if m.warehouse.is_empty() { "Principale" } else { m.warehouse.as_str() },
                dec_to_sql(m.unit_value),
                m.reason,
                m.document_id,
                m.line_id,
                m.lot.trim(),
                m.variant.trim(),
                m.expiry.map(date_to_sql),
                m.serial.trim()
            ],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    fn write_variant_barcodes(&self, p: &Product) -> Result<()> {
        self.conn().execute("DELETE FROM product_barcodes WHERE product_id = ?1", [p.id])?;
        for b in &p.variants.barcodes {
            self.conn().execute(
                "INSERT INTO product_barcodes(barcode, product_id, variant) VALUES (?1, ?2, ?3)",
                params![b.barcode.trim(), p.id, b.variant],
            )?;
        }
        Ok(())
    }

    /// Movimento manuale (rettifica inventario, carico iniziale, ...).
    pub fn add_manual_stock_move(&self, m: &StockMove) -> Result<i64> {
        if m.qty.is_zero() {
            return Err(StoreError::Invalid("Quantità nulla".into()));
        }
        if self.product(m.product_id)?.is_none() {
            return Err(StoreError::NotFound("articolo".into()));
        }
        let mut m = m.clone();
        m.document_id = None;
        m.line_id = None;
        let product = self.product(m.product_id)?.ok_or_else(|| StoreError::NotFound("articolo".into()))?;
        let serials: Vec<String> = if m.serial.trim().is_empty() { vec![] } else { m.serial.split([',', ';', '\n']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect() };
        m.serial.clear();
        let tx = self.begin()?;
        let moves = self.post_tracked_move(&product, m.clone(), &serials, 0)?;
        let first = moves.first().map(|x| x.id).unwrap_or(0);
        self.audit("stock", first, "manual", &format!("articolo {} qta {} magazzino {}", m.product_id, m.qty, m.warehouse))?;
        tx.commit()?;
        Ok(first)
    }

    /// Imposta la giacenza a un valore contato (inventario), generando la rettifica.
    pub fn set_inventory(&self, product_id: i64, counted: Decimal, date: chrono::NaiveDate, warehouse: &str) -> Result<Option<i64>> {
        let warehouse = self.resolve_warehouse(warehouse)?;
        let current = self.stock_in(product_id, &warehouse)?;
        let diff = counted - current;
        if diff.is_zero() {
            return Ok(None);
        }
        let product = self.product(product_id)?.ok_or_else(|| StoreError::NotFound("articolo".into()))?;
        if product.tracking != Tracking::None || !product.variants.is_empty() {
            return Err(StoreError::Invalid("Per articoli con varianti, lotti o seriali registrare carichi e scarichi indicando variante/lotto/seriale".into()));
        }
        let m = StockMove { date, product_id, qty: diff, reason: "Rettifica inventario".into(), warehouse, ..Default::default() };
        Ok(Some(self.add_manual_stock_move(&m)?))
    }

    pub fn delete_manual_stock_move(&self, id: i64) -> Result<()> {
        let n = self.conn().execute("DELETE FROM stock_moves WHERE id = ?1 AND document_id IS NULL", [id])?;
        if n == 0 {
            return Err(StoreError::Invalid("Solo i movimenti manuali si eliminano qui; gli altri seguono il documento".into()));
        }
        Ok(())
    }
}
