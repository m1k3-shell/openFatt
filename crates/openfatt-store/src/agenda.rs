//! Agenda: appuntamenti, telefonate e attività, anche legati a un cliente o fornitore.
//! Carte fedeltà (punti) e buoni regalo / gift card (credito spendibile).

use chrono::NaiveDate;
use rusqlite::{params, OptionalExtension};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{date_from_sql, date_to_sql, dec_from_sql, dec_to_sql, Result, Store, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AgendaKind {
    #[default]
    Appointment,
    Call,
    Task,
    Deadline,
}

impl AgendaKind {
    pub const ALL: [AgendaKind; 4] = [AgendaKind::Appointment, AgendaKind::Call, AgendaKind::Task, AgendaKind::Deadline];
    pub fn label(self) -> &'static str {
        match self {
            AgendaKind::Appointment => "Appuntamento",
            AgendaKind::Call => "Telefonata",
            AgendaKind::Task => "Attività",
            AgendaKind::Deadline => "Scadenza",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgendaItem {
    pub id: i64,
    pub date: NaiveDate,
    /// "09:30" (vuoto = tutto il giorno)
    pub time: String,
    pub kind: AgendaKind,
    pub title: String,
    pub notes: String,
    pub party_id: Option<i64>,
    pub document_id: Option<i64>,
    pub done: bool,
    /// Utente a cui è assegnato (vuoto = tutti)
    pub assigned_to: String,
}

impl Default for AgendaItem {
    fn default() -> Self {
        AgendaItem { id: 0, date: chrono::Local::now().date_naive(), time: String::new(), kind: AgendaKind::Appointment, title: String::new(), notes: String::new(), party_id: None, document_id: None, done: false, assigned_to: String::new() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CardKind {
    #[default]
    Loyalty,
    Gift,
}

impl CardKind {
    pub fn code(self) -> &'static str {
        match self {
            CardKind::Loyalty => "F",
            CardKind::Gift => "B",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            CardKind::Loyalty => "Carta fedeltà",
            CardKind::Gift => "Buono regalo / gift card",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Card {
    pub id: i64,
    pub code: String,
    pub kind: CardKind,
    pub party_id: Option<i64>,
    pub holder: String,
    pub issued: Option<NaiveDate>,
    pub expiry: Option<NaiveDate>,
    pub active: bool,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CardMove {
    pub id: i64,
    pub date: NaiveDate,
    pub points: Decimal,
    pub amount: Decimal,
    pub document_id: Option<i64>,
    pub note: String,
}

/// Regole della raccolta punti.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoyaltySettings {
    /// Punti per ogni euro speso
    pub points_per_euro: Decimal,
    /// Sconto in euro per ogni punto riscattato
    pub euro_per_point: Decimal,
    /// Punti minimi per riscattare
    pub min_redeem: Decimal,
}

impl Default for LoyaltySettings {
    fn default() -> Self {
        LoyaltySettings { points_per_euro: Decimal::ONE, euro_per_point: Decimal::new(5, 2), min_redeem: Decimal::from(100) }
    }
}

impl Store {
    pub fn agenda(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<AgendaItem>> {
        let mut st = self.conn().prepare("SELECT id, data FROM agenda WHERE date >= ?1 AND date <= ?2 ORDER BY date, time, id")?;
        let rows = st.query_map(params![date_to_sql(from), date_to_sql(to)], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, data) = row?;
            let mut a: AgendaItem = serde_json::from_str(&data)?;
            a.id = id;
            out.push(a);
        }
        Ok(out)
    }

    /// Attività non concluse con data passata (da recuperare).
    pub fn agenda_overdue(&self, today: NaiveDate) -> Result<Vec<AgendaItem>> {
        Ok(self.agenda(NaiveDate::from_ymd_opt(2000, 1, 1).unwrap(), today.pred_opt().unwrap_or(today))?.into_iter().filter(|a| !a.done && a.kind != AgendaKind::Appointment).collect())
    }

    pub fn save_agenda_item(&self, a: &AgendaItem) -> Result<AgendaItem> {
        if a.title.trim().is_empty() {
            return Err(StoreError::Invalid("Indicare l'oggetto".into()));
        }
        if !a.time.is_empty() && chrono::NaiveTime::parse_from_str(&a.time, "%H:%M").is_err() {
            return Err(StoreError::Invalid("Ora non valida (formato 09:30)".into()));
        }
        let mut a = a.clone();
        let data = serde_json::to_string(&a)?;
        if a.id == 0 {
            self.conn().execute("INSERT INTO agenda(date, time, done, party_id, data) VALUES (?1, ?2, ?3, ?4, ?5)", params![date_to_sql(a.date), a.time, a.done as i64, a.party_id, data])?;
            a.id = self.conn().last_insert_rowid();
        } else {
            self.conn().execute("UPDATE agenda SET date=?1, time=?2, done=?3, party_id=?4, data=?5 WHERE id=?6", params![date_to_sql(a.date), a.time, a.done as i64, a.party_id, data, a.id])?;
        }
        self.audit("agenda", a.id, "save", &a.title)?;
        Ok(a)
    }

    pub fn delete_agenda_item(&self, id: i64) -> Result<()> {
        self.conn().execute("DELETE FROM agenda WHERE id = ?1", [id])?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Carte fedeltà e buoni
    // ------------------------------------------------------------------

    pub fn cards(&self, kind: Option<CardKind>, search: &str) -> Result<Vec<Card>> {
        let like = format!("%{}%", search.trim());
        let mut st = self.conn().prepare("SELECT id, code, data FROM cards WHERE (?1 = '' OR kind = ?1) AND (?2 = '%%' OR code LIKE ?2 OR data LIKE ?2) ORDER BY id DESC")?;
        let rows = st.query_map(params![kind.map(|k| k.code()).unwrap_or(""), like], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, code, data) = row?;
            let mut c: Card = serde_json::from_str(&data)?;
            c.id = id;
            c.code = code;
            out.push(c);
        }
        Ok(out)
    }

    pub fn card_by_code(&self, code: &str) -> Result<Option<Card>> {
        let row: Option<(i64, String)> = self.conn().query_row("SELECT id, data FROM cards WHERE code = ?1", [code.trim()], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        Ok(match row {
            Some((id, data)) => {
                let mut c: Card = serde_json::from_str(&data)?;
                c.id = id;
                c.code = code.trim().to_string();
                Some(c)
            }
            None => None,
        })
    }

    pub fn save_card(&self, c: &Card) -> Result<Card> {
        let mut c = c.clone();
        c.code = c.code.trim().to_string();
        if c.code.is_empty() {
            c.code = format!("{}{:08}", if c.kind == CardKind::Gift { "GC" } else { "FD" }, self.next_counter(&format!("card_{}", c.kind.code()))?);
        }
        let data = serde_json::to_string(&c)?;
        if c.id == 0 {
            self.conn()
                .execute("INSERT INTO cards(code, kind, party_id, active, data) VALUES (?1, ?2, ?3, ?4, ?5)", params![c.code, c.kind.code(), c.party_id, c.active as i64, data])
                .map_err(|e| match e {
                    rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => StoreError::Invalid(format!("Esiste già una carta con codice {}", c.code)),
                    other => other.into(),
                })?;
            c.id = self.conn().last_insert_rowid();
        } else {
            self.conn().execute("UPDATE cards SET code=?1, kind=?2, party_id=?3, active=?4, data=?5 WHERE id=?6", params![c.code, c.kind.code(), c.party_id, c.active as i64, data, c.id])?;
        }
        self.audit("card", c.id, "save", &c.code)?;
        Ok(c)
    }

    pub fn card_moves(&self, card_id: i64) -> Result<Vec<CardMove>> {
        let mut st = self.conn().prepare("SELECT id, date, points, amount, document_id, note FROM card_moves WHERE card_id = ?1 ORDER BY date, id")?;
        let rows = st.query_map([card_id], |r| {
            Ok(CardMove { id: r.get(0)?, date: date_from_sql(&r.get::<_, String>(1)?), points: dec_from_sql(r.get(2)?), amount: dec_from_sql(r.get(3)?), document_id: r.get(4)?, note: r.get(5)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Saldo della carta: (punti, credito in euro).
    pub fn card_balance(&self, card_id: i64) -> Result<(Decimal, Decimal)> {
        let m = self.card_moves(card_id)?;
        Ok((m.iter().map(|x| x.points).sum(), m.iter().map(|x| x.amount).sum()))
    }

    /// Registra un movimento: punti o credito positivi = accredito, negativi = utilizzo.
    /// Il saldo non può andare sotto zero e una carta scaduta o disattivata non si usa.
    pub fn add_card_move(&self, card_id: i64, date: NaiveDate, points: Decimal, amount: Decimal, document_id: Option<i64>, note: &str) -> Result<i64> {
        let card = self.cards(None, "")?.into_iter().find(|c| c.id == card_id).ok_or_else(|| StoreError::NotFound("carta".into()))?;
        if (points < Decimal::ZERO || amount < Decimal::ZERO) && (!card.active || card.expiry.map(|e| e < date).unwrap_or(false)) {
            return Err(StoreError::Invalid(format!("La carta {} è scaduta o non attiva", card.code)));
        }
        let (p, a) = self.card_balance(card_id)?;
        if p + points < Decimal::ZERO {
            return Err(StoreError::Invalid(format!("Punti insufficienti: disponibili {}", p.normalize())));
        }
        if a + amount < Decimal::ZERO {
            return Err(StoreError::Invalid(format!("Credito insufficiente: disponibili € {}", openfatt_core::money::fmt_it(a, 2))));
        }
        self.conn().execute(
            "INSERT INTO card_moves(card_id, date, points, amount, document_id, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![card_id, date_to_sql(date), dec_to_sql(points), dec_to_sql(amount), document_id, note],
        )?;
        let id = self.conn().last_insert_rowid();
        self.audit("card", card_id, "move", &format!("punti {} importo {} {}", points, amount, note))?;
        Ok(id)
    }

    /// Punti maturati con una spesa, secondo le regole della raccolta.
    pub fn loyalty_points_for(&self, spent: Decimal) -> Result<Decimal> {
        let s: LoyaltySettings = self.get_setting("loyalty")?;
        Ok((spent * s.points_per_euro).floor().max(Decimal::ZERO))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;

    #[test]
    fn agenda_crud_and_overdue() {
        let s = Store::open_in_memory().unwrap();
        let d = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        s.save_agenda_item(&AgendaItem { date: d, time: "09:30".into(), title: "Visita cliente".into(), ..Default::default() }).unwrap();
        s.save_agenda_item(&AgendaItem { date: d, kind: AgendaKind::Call, title: "Richiamare Rossi".into(), ..Default::default() }).unwrap();
        assert!(s.save_agenda_item(&AgendaItem { date: d, time: "25:00".into(), title: "x".into(), ..Default::default() }).is_err());
        assert_eq!(s.agenda(d, d).unwrap().len(), 2);
        let late = s.agenda_overdue(NaiveDate::from_ymd_opt(2026, 11, 7).unwrap()).unwrap();
        assert_eq!(late.len(), 1, "gli appuntamenti passati non sono attività da recuperare");
    }

    #[test]
    fn gift_cards_and_loyalty_points() {
        let s = Store::open_in_memory().unwrap();
        let d = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        let g = s.save_card(&Card { kind: CardKind::Gift, active: true, expiry: NaiveDate::from_ymd_opt(2027, 11, 5), ..Default::default() }).unwrap();
        assert!(g.code.starts_with("GC"));
        s.add_card_move(g.id, d, dec!(0), dec!(50), None, "Emissione").unwrap();
        s.add_card_move(g.id, d, dec!(0), dec!(-30), None, "Utilizzo").unwrap();
        assert!(s.add_card_move(g.id, d, dec!(0), dec!(-30), None, "Troppo").is_err());
        assert_eq!(s.card_balance(g.id).unwrap().1, dec!(20));
        assert!(s.add_card_move(g.id, NaiveDate::from_ymd_opt(2028, 1, 1).unwrap(), dec!(0), dec!(-10), None, "Scaduta").is_err());
        let f = s.save_card(&Card { kind: CardKind::Loyalty, active: true, code: "FD1".into(), ..Default::default() }).unwrap();
        let pts = s.loyalty_points_for(dec!(57.80)).unwrap();
        assert_eq!(pts, dec!(57));
        s.add_card_move(f.id, d, pts, dec!(0), None, "Spesa").unwrap();
        assert!(s.save_card(&Card { code: "FD1".into(), ..Default::default() }).is_err());
        assert_eq!(s.card_by_code("FD1").unwrap().unwrap().id, f.id);
    }
}
