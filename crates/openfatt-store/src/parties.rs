use rusqlite::{params, OptionalExtension};

use openfatt_core::model::{Party, PartyRole};

use crate::{now_sql, Result, Store, StoreError};

#[derive(Debug, Clone, Default)]
pub struct PartyFilter {
    /// None = tutti; Some(Customer) = clienti (compresi "entrambi"), ecc.
    pub role: Option<PartyRole>,
    pub search: String,
    pub include_inactive: bool,
}

impl Store {
    pub fn parties(&self, f: &PartyFilter) -> Result<Vec<Party>> {
        let mut sql = String::from("SELECT id, version, data FROM parties WHERE 1=1");
        match f.role {
            Some(PartyRole::Customer) => sql.push_str(" AND role IN ('C','CF')"),
            Some(PartyRole::Supplier) => sql.push_str(" AND role IN ('F','CF')"),
            Some(PartyRole::Both) => sql.push_str(" AND role = 'CF'"),
            None => {}
        }
        if !f.include_inactive {
            sql.push_str(" AND active = 1");
        }
        let like = format!("%{}%", f.search.trim());
        sql.push_str(" AND (?1 = '%%' OR name LIKE ?1 OR code LIKE ?1 OR vat_number LIKE ?1 OR tax_code LIKE ?1 OR city LIKE ?1 OR email LIKE ?1)");
        sql.push_str(" ORDER BY name COLLATE NOCASE");
        let mut st = self.conn().prepare(&sql)?;
        let rows = st.query_map([like], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, version, data) = row?;
            let mut p: Party = serde_json::from_str(&data)?;
            p.id = id;
            p.version = version;
            out.push(p);
        }
        Ok(out)
    }

    pub fn party(&self, id: i64) -> Result<Option<Party>> {
        let row: Option<(i64, String)> = self
            .conn()
            .query_row("SELECT version, data FROM parties WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        match row {
            None => Ok(None),
            Some((version, data)) => {
                let mut p: Party = serde_json::from_str(&data)?;
                p.id = id;
                p.version = version;
                Ok(Some(p))
            }
        }
    }

    /// Cerca un soggetto per partita IVA o codice fiscale.
    pub fn find_party_by_tax_id(&self, vat: &str, cf: &str) -> Result<Option<Party>> {
        let vat = vat.trim();
        let cf = cf.trim();
        let id: Option<i64> = self
            .conn()
            .query_row(
                "SELECT id FROM parties WHERE (?1 <> '' AND vat_number = ?1) OR (?2 <> '' AND tax_code = ?2) ORDER BY id LIMIT 1",
                params![vat, cf],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.party(id),
            None => Ok(None),
        }
    }

    fn next_party_code(&self, role: PartyRole) -> Result<String> {
        let prefix = if role == PartyRole::Supplier { "F" } else { "C" };
        let n = self.next_counter(&format!("party_code_{}", prefix))?;
        Ok(format!("{}{:05}", prefix, n))
    }

    /// Inserisce o aggiorna. In aggiornamento la versione deve coincidere con quella salvata.
    /// Modifica più soggetti insieme: tutte le modifiche o nessuna.
    pub fn update_parties(&self, ids: &[i64], f: impl Fn(&mut Party)) -> Result<usize> {
        let tx = self.begin()?;
        let mut n = 0;
        for id in ids {
            if let Some(mut p) = self.party(*id)? {
                f(&mut p);
                self.save_party(&p)?;
                n += 1;
            }
        }
        self.audit("party", 0, "bulk_update", &format!("{} soggetti", n))?;
        tx.commit()?;
        Ok(n)
    }

    pub fn save_party(&self, p: &Party) -> Result<Party> {
        let mut p = p.clone();
        if p.display_name().is_empty() {
            return Err(StoreError::Invalid("Indicare la ragione sociale oppure nome e cognome".into()));
        }
        p.vat_number = p.vat_number.trim().replace(' ', "").to_uppercase();
        p.tax_code = p.tax_code.trim().replace(' ', "").to_uppercase();
        p.sdi_code = p.sdi_code.trim().to_uppercase();
        p.province = p.province.trim().to_uppercase();
        if p.code.trim().is_empty() {
            p.code = self.next_party_code(p.role)?;
        }
        let name = p.display_name();
        let data = serde_json::to_string(&p)?;
        if p.id == 0 {
            self.conn().execute(
                "INSERT INTO parties(code, role, name, vat_number, tax_code, city, email, active, version, data, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10)",
                params![p.code, p.role.code(), name, p.vat_number, p.tax_code, p.city, p.email, p.active as i64, data, now_sql()],
            )?;
            p.id = self.conn().last_insert_rowid();
            p.version = 1;
            self.audit("party", p.id, "insert", &name)?;
        } else {
            let n = self.conn().execute(
                "UPDATE parties SET code=?1, role=?2, name=?3, vat_number=?4, tax_code=?5, city=?6, email=?7, active=?8,
                 version = version + 1, data=?9, updated_at=?10 WHERE id=?11 AND version=?12",
                params![p.code, p.role.code(), name, p.vat_number, p.tax_code, p.city, p.email, p.active as i64, data, now_sql(), p.id, p.version],
            )?;
            if n == 0 {
                return Err(StoreError::Conflict(format!("'{}' è stato modificato da un'altra sessione: ricaricare i dati", name)));
            }
            p.version += 1;
            self.audit("party", p.id, "update", &name)?;
        }
        Ok(p)
    }

    pub fn delete_party(&self, id: i64) -> Result<()> {
        let used: i64 = self.conn().query_row("SELECT COUNT(*) FROM documents WHERE party_id = ?1", [id], |r| r.get(0))?;
        if used > 0 {
            return Err(StoreError::Invalid(format!(
                "Il soggetto è usato in {} documenti: non può essere eliminato (si può disattivare)",
                used
            )));
        }
        self.conn().execute("DELETE FROM parties WHERE id = ?1", [id])?;
        self.audit("party", id, "delete", "")?;
        Ok(())
    }
}
