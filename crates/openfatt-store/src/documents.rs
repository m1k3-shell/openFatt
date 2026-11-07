//! Documenti: salvataggio, emissione con numerazione, effetti (scadenze, magazzino),
//! trasformazioni ed evasioni parziali.

use std::collections::{BTreeMap, HashMap};

use chrono::{Datelike, NaiveDate};
use rusqlite::{params, OptionalExtension};
use rust_decimal::Decimal;

use openfatt_core::calc;
use openfatt_core::model::*;

use crate::{date_from_sql, date_to_sql, dec_from_sql, dec_to_sql, now, now_sql, Result, Store, StoreError};

#[derive(Debug, Clone)]
pub struct DocSummary {
    pub id: i64,
    pub kind: DocKind,
    pub status: DocStatus,
    pub number: Option<i64>,
    pub number_label: String,
    pub series: String,
    pub year: i32,
    pub date: NaiveDate,
    pub party_id: Option<i64>,
    pub party_name: String,
    pub external_number: String,
    pub total: Decimal,
    pub net_to_pay: Decimal,
    /// Stato dell'ultima fattura elettronica collegata.
    pub fe_status: Option<String>,
    /// Residuo da incassare/pagare sulle scadenze.
    pub open_amount: Decimal,
}

#[derive(Debug, Clone, Default)]
pub struct DocFilter {
    pub kinds: Vec<DocKind>,
    pub year: Option<i32>,
    pub party_id: Option<i64>,
    pub search: String,
    pub status: Option<DocStatus>,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

/// DDT di provenienza delle righe di una fattura differita.
#[derive(Debug, Clone)]
pub struct DdtSource {
    pub doc_id: i64,
    pub number: String,
    pub date: NaiveDate,
    pub line_indexes: Vec<usize>,
}

impl Store {
    pub fn documents(&self, f: &DocFilter) -> Result<Vec<DocSummary>> {
        let mut sql = String::from(
            "SELECT d.id, d.kind, d.status, d.number, d.series, d.year, d.date, d.party_id, d.party_name, d.external_number,
                    d.total, d.net_to_pay,
                    (SELECT e.status FROM einvoices e WHERE e.document_id = d.id AND e.direction = 'OUT' ORDER BY e.id DESC LIMIT 1)
             FROM documents d WHERE 1=1",
        );
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        if !f.kinds.is_empty() {
            let marks: Vec<String> = f
                .kinds
                .iter()
                .map(|k| {
                    args.push(k.code().to_string().into());
                    format!("?{}", args.len())
                })
                .collect();
            sql.push_str(&format!(" AND d.kind IN ({})", marks.join(",")));
        }
        if let Some(y) = f.year {
            args.push((y as i64).into());
            sql.push_str(&format!(" AND d.year = ?{}", args.len()));
        }
        if let Some(p) = f.party_id {
            args.push(p.into());
            sql.push_str(&format!(" AND d.party_id = ?{}", args.len()));
        }
        if let Some(s) = f.status {
            args.push(s.code().to_string().into());
            sql.push_str(&format!(" AND d.status = ?{}", args.len()));
        }
        if let Some(d) = f.from {
            args.push(date_to_sql(d).into());
            sql.push_str(&format!(" AND d.date >= ?{}", args.len()));
        }
        if let Some(d) = f.to {
            args.push(date_to_sql(d).into());
            sql.push_str(&format!(" AND d.date <= ?{}", args.len()));
        }
        if !f.search.trim().is_empty() {
            args.push(format!("%{}%", f.search.trim()).into());
            let n = args.len();
            sql.push_str(&format!(
                " AND (d.party_name LIKE ?{n} OR CAST(d.number AS TEXT) LIKE ?{n} OR d.external_number LIKE ?{n} OR d.data LIKE ?{n})"
            ));
        }
        sql.push_str(" ORDER BY d.date DESC, d.number DESC, d.id DESC");
        let mut st = self.conn().prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<i64>>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, Option<String>>(11)?,
                r.get::<_, Option<String>>(12)?,
            ))
        })?;
        let open = self.open_amounts_by_doc()?;
        let mut out = Vec::new();
        for row in rows {
            let (id, kind, status, number, series, year, date, party_id, party_name, ext, total, net, fe) = row?;
            let kind = DocKind::from_code(&kind).unwrap_or_default();
            let number_label = match number {
                Some(n) if series.is_empty() => n.to_string(),
                Some(n) => format!("{}/{}", n, series),
                None => "(bozza)".into(),
            };
            out.push(DocSummary {
                id,
                kind,
                status: DocStatus::from_code(&status),
                number,
                number_label,
                series,
                year: year as i32,
                date: date_from_sql(&date),
                party_id,
                party_name: party_name.unwrap_or_default(),
                external_number: ext.unwrap_or_default(),
                total: dec_from_sql(total),
                net_to_pay: dec_from_sql(net),
                fe_status: fe,
                open_amount: open.get(&id).copied().unwrap_or_default(),
            });
        }
        Ok(out)
    }

    fn open_amounts_by_doc(&self) -> Result<HashMap<i64, Decimal>> {
        let mut st = self.conn().prepare("SELECT document_id, amount, paid, writeoff FROM due_dates")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))?;
        let mut m: HashMap<i64, Decimal> = HashMap::new();
        for r in rows {
            let (id, a, p, w) = r?;
            *m.entry(id).or_default() += dec_from_sql(Some(a)) - dec_from_sql(Some(p)) - dec_from_sql(Some(w));
        }
        Ok(m)
    }

    pub fn document(&self, id: i64) -> Result<Document> {
        let (version, data): (i64, String) = self
            .conn()
            .query_row("SELECT version, data FROM documents WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .ok_or_else(|| StoreError::NotFound(format!("documento {}", id)))?;
        let mut d: Document = serde_json::from_str(&data)?;
        d.id = id;
        d.version = version;
        d.due_dates = self.due_dates_of(id)?;
        Ok(d)
    }

    pub(crate) fn due_dates_of(&self, doc_id: i64) -> Result<Vec<DueDate>> {
        let mut st = self.conn().prepare("SELECT id, date, amount, mp_code, paid, paid_date, writeoff FROM due_dates WHERE document_id = ?1 ORDER BY date, id")?;
        let rows = st.query_map([doc_id], |r| {
            Ok(DueDate {
                id: r.get(0)?,
                date: date_from_sql(&r.get::<_, String>(1)?),
                amount: dec_from_sql(r.get(2)?),
                mp_code: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                paid: dec_from_sql(r.get(4)?),
                writeoff: dec_from_sql(r.get(6)?),
                paid_date: r.get::<_, Option<String>>(5)?.map(|s| date_from_sql(&s)),
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Motivo per cui il documento non è più modificabile, se esiste.
    pub fn document_lock_reason(&self, id: i64) -> Result<Option<String>> {
        let kind: Option<String> = self.conn().query_row("SELECT kind FROM documents WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        let year: Option<i32> = self.conn().query_row("SELECT year FROM documents WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        if let Some(y) = year {
            if self.year_is_closed(y)? {
                return Ok(Some(format!("l'anno {} è chiuso (Strumenti > Operazioni di fine anno per riaprirlo)", y)));
            }
        }
        if kind.as_deref() == Some("BAN") {
            let d = self.document(id)?;
            if d.retail.as_ref().map(|r| matches!(r.rt_status, RtStatus::Printed | RtStatus::Manual)).unwrap_or(false) {
                return Ok(Some("documento commerciale emesso dal registratore: per correggere registrare un reso".into()));
            }
        }
        let fe: Option<String> = self
            .conn()
            .query_row(
                "SELECT status FROM einvoices WHERE document_id = ?1 AND direction = 'OUT' AND status NOT IN ('DA_INVIARE','SCARTATA') ORDER BY id DESC LIMIT 1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(s) = fe {
            return Ok(Some(format!("fattura elettronica già trasmessa (stato {}): per correggerla emettere una nota di credito", s)));
        }
        let paid: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM due_dates WHERE document_id = ?1 AND (paid <> '0' OR writeoff <> '0')",
            [id],
            |r| r.get(0),
        )?;
        if paid > 0 {
            return Ok(Some("ci sono pagamenti registrati sulle scadenze: annullarli prima di modificare".into()));
        }
        Ok(None)
    }

    fn assign_line_ids(&self, doc: &mut Document) -> Result<()> {
        for l in doc.lines.iter_mut() {
            if l.id == 0 {
                l.id = self.next_counter("line_id")? as i64 + 1_000_000;
            }
        }
        Ok(())
    }

    fn write_document_row(&self, doc: &mut Document, totals: Option<&calc::Totals>) -> Result<()> {
        doc.year = doc.date.year();
        doc.updated_at = Some(now());
        if doc.created_at.is_none() {
            doc.created_at = doc.updated_at;
        }
        let mut stored = doc.clone();
        stored.due_dates.clear();
        let data = serde_json::to_string(&stored)?;
        let total = totals.map(|t| dec_to_sql(t.total)).unwrap_or_else(|| "0".into());
        let net = totals.map(|t| dec_to_sql(t.net_to_pay)).unwrap_or_else(|| "0".into());
        let party_name = doc.party.display_name();
        let register = doc.kind.numbering_register();
        if doc.id == 0 {
            self.conn().execute(
                "INSERT INTO documents(kind, status, register, series, year, number, date, party_id, party_name, external_number,
                 total, net_to_pay, version, data, created_at, updated_at, agent_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?14, ?14, ?15)",
                params![
                    doc.kind.code(),
                    doc.status.code(),
                    register,
                    doc.series.trim(),
                    doc.year,
                    doc.number,
                    date_to_sql(doc.date),
                    doc.party_id,
                    party_name,
                    doc.external_number,
                    total,
                    net,
                    data,
                    now_sql(),
                    doc.agent_id
                ],
            )
            .map_err(map_unique)?;
            doc.id = self.conn().last_insert_rowid();
            doc.version = 1;
        } else {
            let n = self
                .conn()
                .execute(
                    "UPDATE documents SET kind=?1, status=?2, register=?3, series=?4, year=?5, number=?6, date=?7, party_id=?8,
                     party_name=?9, external_number=?10, total=?11, net_to_pay=?12, version=version+1, data=?13, updated_at=?14, agent_id=?17
                     WHERE id=?15 AND version=?16",
                    params![
                        doc.kind.code(),
                        doc.status.code(),
                        register,
                        doc.series.trim(),
                        doc.year,
                        doc.number,
                        date_to_sql(doc.date),
                        doc.party_id,
                        party_name,
                        doc.external_number,
                        total,
                        net,
                        data,
                        now_sql(),
                        doc.id,
                        doc.version,
                        doc.agent_id
                    ],
                )
                .map_err(map_unique)?;
            if n == 0 {
                return Err(StoreError::Conflict("il documento è stato modificato in un'altra finestra o postazione: riaprirlo".into()));
            }
            doc.version += 1;
        }
        // indice delle righe (per evasioni e magazzino)
        self.conn().execute("DELETE FROM doc_lines WHERE document_id = ?1", [doc.id])?;
        for (i, l) in doc.lines.iter().enumerate() {
            self.conn().execute(
                "INSERT INTO doc_lines(id, document_id, position, product_id, qty, source_line_id, source_doc_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![l.id, doc.id, i as i64, l.product_id, dec_to_sql(l.qty), l.source_line_id, l.source_doc_id],
            )?;
        }
        Ok(())
    }

    /// Salva un documento (nuovo o esistente). Se è già emesso, ne rigenera gli effetti.
    pub fn save_document(&self, doc: &Document) -> Result<Document> {
        let mut doc = doc.clone();
        if doc.id != 0 {
            if let Some(reason) = self.document_lock_reason(doc.id)? {
                return Err(StoreError::Invalid(format!("Documento non modificabile: {}", reason)));
            }
        }
        if let Some(pid) = doc.party_id {
            if doc.party.id != pid {
                if let Some(p) = self.party(pid)? {
                    doc.party = p;
                }
            }
        }
        let tx = self.begin()?;
        self.assign_line_ids(&mut doc)?;
        let totals = calc::compute(&doc).ok();
        let issued = doc.status == DocStatus::Issued;
        self.write_document_row(&mut doc, totals.as_ref())?;
        if issued {
            self.apply_effects(&mut doc, totals.as_ref())?;
        }
        self.audit("document", doc.id, "save", &doc.title())?;
        tx.commit()?;
        doc.due_dates = self.due_dates_of(doc.id)?;
        Ok(doc)
    }

    fn clear_effects(&self, doc_id: i64) -> Result<()> {
        self.conn().execute("DELETE FROM stock_moves WHERE document_id = ?1", [doc_id])?;
        self.conn().execute("DELETE FROM due_dates WHERE document_id = ?1", [doc_id])?;
        Ok(())
    }

    fn kind_of(&self, doc_id: i64) -> Result<Option<DocKind>> {
        let k: Option<String> = self.conn().query_row("SELECT kind FROM documents WHERE id = ?1", [doc_id], |r| r.get(0)).optional()?;
        Ok(k.and_then(|k| DocKind::from_code(&k)))
    }

    /// Scadenze e movimenti di magazzino di un documento emesso.
    fn apply_effects(&self, doc: &mut Document, totals: Option<&calc::Totals>) -> Result<()> {
        self.clear_effects(doc.id)?;
        if doc.kind == DocKind::StockTransfer {
            return self.apply_transfer(doc);
        }
        let totals = match totals {
            Some(t) => Some(t.clone()),
            None if doc.kind.is_internal() => None,
            None => Some(calc::compute(doc).map_err(|e| StoreError::Invalid(e.0))?),
        };
        // scadenze
        if let Some(totals) = totals.filter(|t| doc.kind.has_due_dates() && !t.net_to_pay.is_zero()) {
            let payment = doc.payment.clone().unwrap_or_default();
            let dues = calc::due_dates(doc.date, totals.net_to_pay, &payment);
            for d in &dues {
                self.conn().execute(
                    "INSERT INTO due_dates(document_id, date, amount, mp_code, paid) VALUES (?1, ?2, ?3, ?4, '0')",
                    params![doc.id, date_to_sql(d.date), dec_to_sql(d.amount), d.mp_code],
                )?;
            }
        }
        // magazzino
        let warehouse = self.resolve_warehouse(&doc.warehouse)?;
        let effect = doc.kind.stock_effect();
        if effect != 0 {
            let mut source_kinds: HashMap<i64, Option<DocKind>> = HashMap::new();
            for (pos, l) in doc.lines.iter().enumerate().filter(|(_, l)| l.kind == LineKind::Item) {
                let Some(pid) = l.product_id else { continue };
                let Some(product) = self.product(pid)? else { continue };
                if !product.manage_stock {
                    continue;
                }
                // una fattura che evade un DDT non scarica di nuovo la merce
                if let Some(src) = l.source_doc_id {
                    let k = match source_kinds.get(&src) {
                        Some(k) => *k,
                        None => {
                            let k = self.kind_of(src)?;
                            source_kinds.insert(src, k);
                            k
                        }
                    };
                    if k.map(|k| k.stock_effect() == effect).unwrap_or(false) {
                        continue;
                    }
                }
                let unit_value = if doc.kind.is_purchase() {
                    calc::apply_cascade(l.unit_price, &calc::parse_discounts(&l.discounts).unwrap_or_default())
                } else if doc.kind == DocKind::Production && !product.components.is_empty() {
                    self.bom_cost(&product)?
                } else {
                    product.cost
                };
                let base = StockMove {
                    id: 0,
                    date: doc.date,
                    product_id: pid,
                    qty: l.qty * Decimal::from(effect),
                    warehouse: warehouse.clone(),
                    unit_value,
                    reason: format!("{} {}", doc.kind.label(), doc.number_label()),
                    document_id: Some(doc.id),
                    line_id: Some(l.id),
                    lot: l.lot.trim().to_string(),
                    variant: l.variant.trim().to_string(),
                    expiry: l.lot_expiry,
                    serial: String::new(),
                };
                let reason = base.reason.clone();
                self.post_tracked_move(&product, base, &l.serials, pos + 1)?;
                // produzione: si scaricano i componenti della distinta base
                if doc.kind == DocKind::Production {
                    for (cp, q) in self.production_components(&product, l.qty)? {
                        if !cp.manage_stock || q.is_zero() {
                            continue;
                        }
                        let m = StockMove {
                            id: 0,
                            date: doc.date,
                            product_id: cp.id,
                            qty: -q,
                            warehouse: warehouse.clone(),
                            unit_value: cp.cost,
                            reason: format!("{} - componente di {}", reason, product.code),
                            document_id: Some(doc.id),
                            line_id: Some(l.id),
                            lot: String::new(),
                            variant: String::new(),
                            expiry: None,
                            serial: String::new(),
                        };
                        self.post_tracked_move(&cp, m, &[], pos + 1)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Prossimo numero libero nel registro/serie/anno.
    pub fn next_number(&self, kind: DocKind, series: &str, year: i32) -> Result<i64> {
        let max: Option<i64> = self.conn().query_row(
            "SELECT MAX(number) FROM documents WHERE register = ?1 AND series = ?2 AND year = ?3",
            params![kind.numbering_register(), series.trim(), year],
            |r| r.get(0),
        )?;
        Ok(max.unwrap_or(0) + 1)
    }

    /// Emette il documento: numera, storicizza i dati, genera scadenze e movimenti.
    pub fn issue_document(&self, id: i64) -> Result<Document> {
        let mut doc = self.document(id)?;
        if doc.status == DocStatus::Cancelled {
            return Err(StoreError::Invalid("Documento annullato".into()));
        }
        if doc.status == DocStatus::Issued {
            return Ok(doc);
        }
        // dati storicizzati aggiornati al momento dell'emissione
        match doc.party_id {
            Some(pid) => {
                if let Some(p) = self.party(pid)? {
                    doc.party = p;
                }
            }
            None if doc.kind.is_internal() => {}
            None => return Err(StoreError::Invalid("Selezionare il cliente/fornitore prima di emettere".into())),
        }
        if let Some(pay) = doc.payment_id {
            doc.payment = self.payment_method(pay)?;
        }
        if doc.lines.is_empty() {
            return Err(StoreError::Invalid("Il documento non ha righe".into()));
        }
        let totals = if doc.kind.is_internal() {
            calc::compute(&doc).ok()
        } else {
            Some(calc::compute(&doc).map_err(|e| StoreError::Invalid(e.0))?)
        };
        let year = doc.date.year();
        doc.year = year;

        let tx = self.begin()?;
        let number = match doc.number {
            Some(n) => n,
            None => self.next_number(doc.kind, &doc.series, year)?,
        };
        if doc.kind.is_fiscal() {
            // numerazione progressiva per anno con date non decrescenti
            // la numerazione è sempre tenuta in ordine di data: basta confrontare il numero
            // precedente e il successivo (due letture sull'indice, anche con molte fatture)
            let reg = doc.kind.numbering_register();
            let prev: Option<(i64, String)> = self
                .conn()
                .query_row(
                    "SELECT number, date FROM documents WHERE register = ?1 AND series = ?2 AND year = ?3 AND id <> ?4
                     AND number IS NOT NULL AND number < ?5 ORDER BY number DESC LIMIT 1",
                    params![reg, doc.series.trim(), year, doc.id, number],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let next: Option<(i64, String)> = self
                .conn()
                .query_row(
                    "SELECT number, date FROM documents WHERE register = ?1 AND series = ?2 AND year = ?3 AND id <> ?4
                     AND number IS NOT NULL AND number > ?5 ORDER BY number ASC LIMIT 1",
                    params![reg, doc.series.trim(), year, doc.id, number],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let d = date_to_sql(doc.date);
            let later = prev.filter(|(_, pd)| *pd > d).or(next.filter(|(_, nd)| *nd < d));
            if let Some((n, d)) = later {
                return Err(StoreError::Invalid(format!(
                    "Numerazione non cronologica: il documento n. {} ha data {}. Correggere data o numero.",
                    n,
                    date_from_sql(&d).format("%d/%m/%Y")
                )));
            }
        }
        doc.number = Some(number);
        doc.status = DocStatus::Issued;
        self.write_document_row(&mut doc, totals.as_ref())?;
        self.apply_effects(&mut doc, totals.as_ref())?;
        self.audit("document", doc.id, "issue", &doc.title())?;
        tx.commit()?;
        self.document(id)
    }

    /// Riporta in bozza un documento emesso (se non trasmesso e senza pagamenti).
    pub fn revert_to_draft(&self, id: i64) -> Result<Document> {
        if let Some(r) = self.document_lock_reason(id)? {
            return Err(StoreError::Invalid(format!("Impossibile riportare in bozza: {}", r)));
        }
        let used = self.children_count(id)?;
        if used > 0 {
            return Err(StoreError::Invalid(format!("Il documento è stato evaso/trasformato in {} documenti", used)));
        }
        let mut doc = self.document(id)?;
        let tx = self.begin()?;
        self.clear_effects(id)?;
        doc.status = DocStatus::Draft;
        // il numero resta riservato per evitare buchi nella numerazione
        let totals = calc::compute(&doc).ok();
        self.write_document_row(&mut doc, totals.as_ref())?;
        self.audit("document", id, "revert", &doc.title())?;
        tx.commit()?;
        self.document(id)
    }

    fn children_count(&self, id: i64) -> Result<i64> {
        Ok(self.conn().query_row(
            "SELECT COUNT(DISTINCT l.document_id) FROM doc_lines l JOIN documents d ON d.id = l.document_id
             WHERE l.source_doc_id = ?1 AND d.status <> 'ANNULLATO'",
            [id],
            |r| r.get(0),
        )?)
    }

    /// Annulla: il documento resta (con il suo numero) ma non produce effetti.
    pub fn cancel_document(&self, id: i64) -> Result<Document> {
        if let Some(r) = self.document_lock_reason(id)? {
            return Err(StoreError::Invalid(format!("Impossibile annullare: {}", r)));
        }
        let mut doc = self.document(id)?;
        let tx = self.begin()?;
        self.clear_effects(id)?;
        doc.status = DocStatus::Cancelled;
        let totals = calc::compute(&doc).ok();
        self.write_document_row(&mut doc, totals.as_ref())?;
        self.audit("document", id, "cancel", &doc.title())?;
        tx.commit()?;
        self.document(id)
    }

    /// Elimina un documento. I documenti fiscali numerati si eliminano solo se sono
    /// l'ultimo numero del registro e non sono stati trasmessi.
    pub fn delete_document(&self, id: i64) -> Result<()> {
        if let Some(r) = self.document_lock_reason(id)? {
            return Err(StoreError::Invalid(format!("Impossibile eliminare: {}", r)));
        }
        if self.children_count(id)? > 0 {
            return Err(StoreError::Invalid("Il documento è stato evaso in altri documenti".into()));
        }
        let doc = self.document(id)?;
        if doc.kind.is_fiscal() {
            if let Some(n) = doc.number {
                let next = self.next_number(doc.kind, &doc.series, doc.year)?;
                if n + 1 != next {
                    return Err(StoreError::Invalid(
                        "Non è l'ultimo numero del registro: eliminarlo creerebbe un buco nella numerazione. Usare 'Annulla'.".into(),
                    ));
                }
            }
        }
        let tx = self.begin()?;
        self.conn().execute("DELETE FROM einvoices WHERE document_id = ?1 AND direction = 'OUT' AND status IN ('DA_INVIARE')", [id])?;
        self.conn().execute("UPDATE einvoices SET document_id = NULL WHERE document_id = ?1", [id])?;
        self.conn().execute("DELETE FROM documents WHERE id = ?1", [id])?;
        self.audit("document", id, "delete", &doc.title())?;
        tx.commit()?;
        Ok(())
    }

    /// Copia un documento in una nuova bozza.
    pub fn duplicate_document(&self, id: i64, kind: Option<DocKind>) -> Result<Document> {
        let src = self.document(id)?;
        let mut d = src.clone();
        d.id = 0;
        d.version = 0;
        d.number = None;
        d.status = DocStatus::Draft;
        d.kind = kind.unwrap_or(src.kind);
        d.date = chrono::Local::now().date_naive();
        d.due_dates.clear();
        d.created_at = None;
        d.external_number.clear();
        for l in d.lines.iter_mut() {
            l.id = 0;
            l.source_line_id = None;
            l.source_doc_id = None;
        }
        if d.kind.has_transport() && d.transport.is_none() {
            d.transport = Some(Transport::default());
        }
        self.save_document(&d)
    }

    /// Quantità già evasa di una riga (somma delle righe derivate non annullate).
    pub fn fulfilled_qty(&self, line_id: i64) -> Result<Decimal> {
        let mut st = self.conn().prepare(
            "SELECT l.qty FROM doc_lines l JOIN documents d ON d.id = l.document_id WHERE l.source_line_id = ?1 AND d.status <> 'ANNULLATO'",
        )?;
        let rows = st.query_map([line_id], |r| r.get::<_, String>(0))?;
        let mut t = Decimal::ZERO;
        for r in rows {
            t += dec_from_sql(Some(r?));
        }
        Ok(t)
    }

    /// Stato di evasione: (evaso completamente, evaso parzialmente).
    pub fn fulfillment_state(&self, doc: &Document) -> Result<(bool, bool)> {
        let mut any = false;
        let mut all = true;
        for l in doc.lines.iter().filter(|l| l.kind == LineKind::Item && !l.qty.is_zero()) {
            let f = self.fulfilled_qty(l.id)?;
            if f > Decimal::ZERO {
                any = true;
            }
            if f < l.qty {
                all = false;
            }
        }
        Ok((any && all, any && !all))
    }

    /// Crea una nuova bozza di tipo `target` da uno o più documenti (stesso soggetto),
    /// riportando le quantità residue e i collegamenti riga per riga.
    pub fn transform(&self, source_ids: &[i64], target: DocKind) -> Result<Document> {
        if source_ids.is_empty() {
            return Err(StoreError::Invalid("Nessun documento selezionato".into()));
        }
        let mut sources: Vec<Document> = source_ids.iter().map(|id| self.document(*id)).collect::<Result<_>>()?;
        sources.sort_by(|a, b| a.date.cmp(&b.date).then(a.number.cmp(&b.number)));
        let first = sources[0].clone();
        for s in &sources {
            if s.status == DocStatus::Cancelled {
                return Err(StoreError::Invalid(format!("{} è annullato", s.title())));
            }
            if !s.kind.transform_targets().contains(&target) {
                return Err(StoreError::Invalid(format!("Da {} non si può generare {}", s.kind.label(), target.label())));
            }
            if s.party_id != first.party_id {
                return Err(StoreError::Invalid("I documenti selezionati devono avere lo stesso cliente/fornitore".into()));
            }
            if s.kind == DocKind::DeliveryNote && s.status != DocStatus::Issued {
                return Err(StoreError::Invalid(format!("{} deve essere emesso prima della fatturazione", s.title())));
            }
        }
        let mut d = Document::new(target);
        d.party_id = first.party_id;
        d.party = first.party.clone();
        d.payment_id = first.payment_id;
        d.payment = first.payment.clone();
        d.prices_include_vat = first.prices_include_vat;
        d.discount = first.discount.clone();
        d.pension_fund = first.pension_fund.clone();
        d.withholdings = first.withholdings.clone();
        d.stamp = first.stamp.clone();
        d.vat_liability = first.vat_liability.clone();
        d.series = if target.numbering_register() == first.kind.numbering_register() { first.series.clone() } else { String::new() };
        d.related = sources.iter().flat_map(|s| s.related.iter().filter(|r| r.kind != "FATTURA").cloned()).collect();
        let credit = target.is_credit();
        let all_ddt = sources.iter().all(|s| s.kind == DocKind::DeliveryNote);
        if target == DocKind::Invoice && all_ddt {
            d.fe_type = "TD24".into();
        }
        for s in &sources {
            if credit {
                d.related.push(RelatedDoc { kind: "FATTURA".into(), number: s.number_label(), date: Some(s.date), ..Default::default() });
            }
            if sources.len() > 1 || s.kind == DocKind::DeliveryNote || s.kind == DocKind::GoodsReceipt {
                d.lines.push(DocLine {
                    kind: LineKind::Text,
                    description: format!("Rif. {} n. {} del {}", s.kind.label(), s.number_label(), s.date.format("%d/%m/%Y")),
                    source_doc_id: Some(s.id),
                    ..Default::default()
                });
            }
            for l in &s.lines {
                let mut nl = l.clone();
                nl.id = 0;
                nl.source_doc_id = Some(s.id);
                nl.source_line_id = if l.kind == LineKind::Item { Some(l.id) } else { None };
                if l.kind == LineKind::Item && !credit {
                    let residual = l.qty - self.fulfilled_qty(l.id)?;
                    if residual <= Decimal::ZERO {
                        continue;
                    }
                    nl.qty = residual;
                }
                d.lines.push(nl);
            }
        }
        if d.lines.iter().all(|l| l.kind == LineKind::Text) {
            return Err(StoreError::Invalid("Tutte le righe risultano già evase".into()));
        }
        self.save_document(&d)
    }

    /// DDT d'origine delle righe (per i DatiDDT della fattura elettronica differita).
    pub fn ddt_sources(&self, doc: &Document) -> Result<Vec<DdtSource>> {
        let mut map: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
        for (i, l) in doc.lines.iter().enumerate() {
            if let Some(src) = l.source_doc_id {
                if l.kind == LineKind::Item {
                    map.entry(src).or_default().push(i);
                }
            }
        }
        let mut out = Vec::new();
        for (src, idx) in map {
            if self.kind_of(src)? != Some(DocKind::DeliveryNote) {
                continue;
            }
            let s = self.document(src)?;
            out.push(DdtSource { doc_id: src, number: s.number_label(), date: s.date, line_indexes: idx });
        }
        Ok(out)
    }

    /// Importa un documento storico così com'è (numero, stato emesso, scadenze e pagamenti
    /// del file d'origine). Non applica il controllo cronologico della numerazione.
    pub fn import_document(&self, doc: &Document) -> Result<Document> {
        let mut doc = doc.clone();
        doc.id = 0;
        doc.version = 0;
        if doc.number.is_some() && doc.status == DocStatus::Draft {
            doc.status = DocStatus::Issued;
        }
        let dues = std::mem::take(&mut doc.due_dates);
        let tx = self.begin()?;
        self.assign_line_ids(&mut doc)?;
        let totals = calc::compute(&doc).ok();
        self.write_document_row(&mut doc, totals.as_ref())?;
        if doc.status == DocStatus::Issued {
            self.apply_effects(&mut doc, totals.as_ref())?;
            if !dues.is_empty() {
                self.conn().execute("DELETE FROM due_dates WHERE document_id = ?1", [doc.id])?;
                for d in &dues {
                    self.conn().execute(
                        "INSERT INTO due_dates(document_id, date, amount, mp_code, paid, paid_date) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![doc.id, date_to_sql(d.date), dec_to_sql(d.amount), if d.mp_code.is_empty() { "MP05" } else { d.mp_code.as_str() }, dec_to_sql(d.paid), d.paid_date.map(date_to_sql)],
                    )?;
                }
            }
        }
        self.audit("document", doc.id, "import", &doc.title())?;
        tx.commit()?;
        self.document(doc.id)
    }

    /// Anni presenti in archivio (per i filtri).
    pub fn document_years(&self) -> Result<Vec<i32>> {
        let mut st = self.conn().prepare("SELECT DISTINCT year FROM documents ORDER BY year DESC")?;
        let rows = st.query_map([], |r| r.get::<_, i64>(0))?;
        Ok(rows.filter_map(|r| r.ok()).map(|y| y as i32).collect())
    }
}

impl Store {
    /// Trasferimento: per ogni riga scarico dal magazzino d'origine e carico identico
    /// (stesso lotto, scadenza e seriali) nel magazzino di destinazione, nella stessa transazione.
    fn apply_transfer(&self, doc: &mut Document) -> Result<()> {
        let from = self.resolve_warehouse(&doc.warehouse)?;
        let to = self.resolve_warehouse(&doc.to_warehouse)?;
        if doc.to_warehouse.trim().is_empty() {
            return Err(StoreError::Invalid("Indicare il magazzino di destinazione".into()));
        }
        if from == to {
            return Err(StoreError::Invalid("Il magazzino di destinazione deve essere diverso da quello d'origine".into()));
        }
        for (pos, l) in doc.lines.iter().enumerate().filter(|(_, l)| l.kind == LineKind::Item) {
            let Some(pid) = l.product_id else { continue };
            let Some(product) = self.product(pid)? else { continue };
            if !product.manage_stock {
                continue;
            }
            if l.qty <= Decimal::ZERO {
                return Err(StoreError::Invalid(format!("Riga {}: la quantità da trasferire deve essere positiva", pos + 1)));
            }
            if product.tracking == openfatt_core::model::Tracking::None && self.stock_in(pid, &from)? < l.qty {
                return Err(StoreError::Invalid(format!(
                    "Riga {}: nel magazzino {} di {} ci sono solo {} pezzi",
                    pos + 1,
                    from,
                    product.code,
                    self.stock_in(pid, &from)?.normalize()
                )));
            }
            let base = StockMove {
                id: 0,
                date: doc.date,
                product_id: pid,
                qty: -l.qty,
                warehouse: from.clone(),
                unit_value: product.cost,
                reason: format!("{} {} verso {}", doc.kind.label(), doc.number_label(), to),
                document_id: Some(doc.id),
                line_id: Some(l.id),
                lot: l.lot.trim().to_string(),
                variant: l.variant.trim().to_string(),
                expiry: None,
                serial: String::new(),
            };
            let outs = self.post_tracked_move(&product, base, &l.serials, pos + 1)?;
            for o in outs {
                self.insert_stock_move(&StockMove {
                    id: 0,
                    qty: -o.qty,
                    warehouse: to.clone(),
                    reason: format!("{} {} da {}", doc.kind.label(), doc.number_label(), from),
                    ..o
                })?;
            }
        }
        Ok(())
    }
}

fn map_unique(e: rusqlite::Error) -> StoreError {
    match &e {
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
            StoreError::Invalid("Numero già utilizzato nello stesso registro, serie e anno".into())
        }
        _ => StoreError::Db(e),
    }
}
