//! Registro dei messaggi email e scelta dei clienti da sollecitare.

use std::collections::BTreeMap;

use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::params;
use rust_decimal::Decimal;

use openfatt_core::mailtext::next_reminder_level;

use crate::{now_sql, DueRow, Result, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailKind {
    Document,
    Reminder,
    Communication,
}

impl MailKind {
    pub fn code(self) -> &'static str {
        match self {
            MailKind::Document => "DOCUMENTO",
            MailKind::Reminder => "SOLLECITO",
            MailKind::Communication => "COMUNICAZIONE",
        }
    }
    pub fn from_code(s: &str) -> MailKind {
        match s {
            "SOLLECITO" => MailKind::Reminder,
            "COMUNICAZIONE" => MailKind::Communication,
            _ => MailKind::Document,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            MailKind::Document => "Documento",
            MailKind::Reminder => "Sollecito",
            MailKind::Communication => "Comunicazione",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailStatus {
    /// Spedito dal server SMTP.
    Sent,
    /// Aperto nel programma di posta del computer (l'invio lo conferma l'utente).
    Prepared,
    Failed,
}

impl MailStatus {
    pub fn code(self) -> &'static str {
        match self {
            MailStatus::Sent => "INVIATO",
            MailStatus::Prepared => "PREPARATO",
            MailStatus::Failed => "ERRORE",
        }
    }
    pub fn from_code(s: &str) -> MailStatus {
        match s {
            "INVIATO" => MailStatus::Sent,
            "PREPARATO" => MailStatus::Prepared,
            _ => MailStatus::Failed,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            MailStatus::Sent => "Inviato",
            MailStatus::Prepared => "Aperto nel programma di posta",
            MailStatus::Failed => "Non inviato",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MailLogEntry {
    pub id: i64,
    pub ts: NaiveDateTime,
    pub kind: MailKind,
    pub party_id: Option<i64>,
    pub document_id: Option<i64>,
    /// Livello del sollecito (1-3), 0 per gli altri messaggi.
    pub level: u8,
    pub recipients: String,
    pub subject: String,
    pub status: MailStatus,
    pub detail: String,
    /// Scadenze sollecitate.
    pub due_ids: Vec<i64>,
}

impl Default for MailLogEntry {
    fn default() -> Self {
        MailLogEntry {
            id: 0,
            ts: crate::now(),
            kind: MailKind::Document,
            party_id: None,
            document_id: None,
            level: 0,
            recipients: String::new(),
            subject: String::new(),
            status: MailStatus::Sent,
            detail: String::new(),
            due_ids: Vec::new(),
        }
    }
}

/// Cliente con scadenze da sollecitare.
#[derive(Debug, Clone)]
pub struct ReminderCandidate {
    pub party_id: Option<i64>,
    pub party_name: String,
    pub email: String,
    pub pec: String,
    pub dues: Vec<DueRow>,
    pub overdue_total: Decimal,
    pub oldest_due: NaiveDate,
    pub days_late: i64,
    /// Ultimo sollecito inviato (data e livello).
    pub last_reminder: Option<(NaiveDateTime, u8)>,
    pub next_level: u8,
    /// Un sollecito è già stato inviato da meno dei giorni minimi impostati.
    pub recently_reminded: bool,
}

fn ts_from_sql(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap_or_else(|_| crate::date_from_sql(s).and_hms_opt(0, 0, 0).unwrap())
}

impl Store {
    /// Registra un messaggio inviato (o tentato).
    pub fn log_mail(&self, e: &MailLogEntry) -> Result<i64> {
        let dues = if e.due_ids.is_empty() { None } else { Some(serde_json::to_string(&e.due_ids)?) };
        self.conn().execute(
            "INSERT INTO mail_log(ts, kind, party_id, document_id, level, recipients, subject, status, detail, dues)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![now_sql(), e.kind.code(), e.party_id, e.document_id, e.level as i64, e.recipients, e.subject, e.status.code(), e.detail, dues],
        )?;
        let id = self.conn().last_insert_rowid();
        if e.status != MailStatus::Failed {
            self.audit("mail", id, e.kind.code(), &format!("{} - {}", e.recipients, e.subject))?;
        }
        Ok(id)
    }

    /// Messaggi registrati, i più recenti prima; filtri facoltativi per cliente e documento.
    pub fn mail_log(&self, party_id: Option<i64>, document_id: Option<i64>, limit: usize) -> Result<Vec<MailLogEntry>> {
        let mut st = self.conn().prepare(
            "SELECT id, ts, kind, party_id, document_id, level, recipients, subject, status, detail, dues FROM mail_log
             WHERE (?1 IS NULL OR party_id = ?1) AND (?2 IS NULL OR document_id = ?2)
             ORDER BY id DESC LIMIT ?3",
        )?;
        let rows = st.query_map(params![party_id, document_id, limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, ts, kind, party_id, document_id, level, recipients, subject, status, detail, dues) = row?;
            out.push(MailLogEntry {
                id,
                ts: ts_from_sql(&ts),
                kind: MailKind::from_code(&kind),
                party_id,
                document_id,
                level: level.clamp(0, 3) as u8,
                recipients,
                subject,
                status: MailStatus::from_code(&status),
                detail: detail.unwrap_or_default(),
                due_ids: dues.and_then(|d| serde_json::from_str(&d).ok()).unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// Ultimo invio riuscito di un documento.
    pub fn document_last_mail(&self, document_id: i64) -> Result<Option<MailLogEntry>> {
        Ok(self.mail_log(None, Some(document_id), 20)?.into_iter().find(|e| e.status != MailStatus::Failed && e.kind == MailKind::Document))
    }

    /// Clienti con scadenze aperte scadute da più di `grace_days` giorni, raggruppati per
    /// cliente, con il livello del prossimo sollecito. I più in ritardo prima.
    pub fn reminder_candidates(&self, today: NaiveDate, grace_days: u32, min_days_between: u32) -> Result<Vec<ReminderCandidate>> {
        let limit = today - chrono::Duration::days(grace_days as i64);
        let dues: Vec<DueRow> = self.due_list(true, None)?.into_iter().filter(|d| d.receivable && d.date < limit && d.residual() > Decimal::ZERO).collect();
        let mut groups: BTreeMap<(Option<i64>, String), Vec<DueRow>> = BTreeMap::new();
        for d in dues {
            let key = (d.party_id, if d.party_id.is_some() { String::new() } else { d.party_name.clone() });
            groups.entry(key).or_default().push(d);
        }
        // solleciti già inviati, per cliente
        let mut sent: BTreeMap<i64, Vec<(NaiveDateTime, u8)>> = BTreeMap::new();
        {
            let mut st = self.conn().prepare("SELECT party_id, ts, level FROM mail_log WHERE kind = 'SOLLECITO' AND status <> 'ERRORE' AND party_id IS NOT NULL")?;
            let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?;
            for r in rows {
                let (p, ts, l) = r?;
                sent.entry(p).or_default().push((ts_from_sql(&ts), l.clamp(0, 3) as u8));
            }
        }
        let mut out = Vec::new();
        for ((party_id, _), dues) in groups {
            let party = match party_id {
                Some(id) => self.party(id)?,
                None => None,
            };
            let oldest = dues.iter().map(|d| d.date).min().unwrap_or(today);
            let history = party_id.and_then(|p| sent.get(&p)).cloned().unwrap_or_default();
            let dates: Vec<NaiveDate> = history.iter().map(|(t, _)| t.date()).collect();
            let last = history.iter().max_by_key(|(t, _)| *t).copied();
            let recently = last.is_some_and(|(t, _)| (today - t.date()).num_days() < min_days_between as i64);
            out.push(ReminderCandidate {
                party_id,
                party_name: party.as_ref().map(|p| p.display_name()).unwrap_or_else(|| dues[0].party_name.clone()),
                email: party.as_ref().map(|p| p.email.trim().to_string()).unwrap_or_default(),
                pec: party.as_ref().map(|p| p.pec.trim().to_string()).unwrap_or_default(),
                overdue_total: dues.iter().map(|d| d.residual()).sum(),
                oldest_due: oldest,
                days_late: (today - oldest).num_days(),
                last_reminder: last,
                next_level: next_reminder_level(&dates, oldest),
                recently_reminded: recently,
                dues,
            });
        }
        out.sort_by(|a, b| b.days_late.cmp(&a.days_late).then(b.overdue_total.cmp(&a.overdue_total)));
        Ok(out)
    }
}
