//! Piano dei conti, giroconti, saldi iniziali, riconciliazione bancaria, versamenti F24 ed
//! esportazione per il commercialista.
//!
//! La prima nota di openFatt è per conti finanziari (cassa, banche, carte: le "risorse")
//! e per voci di entrata e di uscita (le categorie dei movimenti). Il piano dei conti dà a
//! ognuno un codice, un gruppo e il codice del conto nel programma dello studio
//! (transcodifica); allo stesso modo aliquote e nature IVA hanno il codice IVA dello studio.
//! I tracciati dei programmi per commercialisti sono proprietari: l'esportazione è un CSV
//! con i codici transcodificati, che lo studio importa con il proprio tracciato libero.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use rusqlite::{params, OptionalExtension};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use openfatt_core::model::CashEntry;
use openfatt_core::vat::{Period, Register};

use crate::{date_to_sql, dec_from_sql, dec_to_sql, Allocation, NewPayment, Result, Store, StoreError};

/// Voce dei movimenti dei giroconti (non è né entrata né uscita).
pub const TRANSFER: &str = "Giroconto";
/// Voce del saldo iniziale di un conto.
pub const OPENING: &str = "Saldo iniziale";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum AccountKind {
    /// Cassa, banca, carta, conto PayPal...
    #[default]
    Resource,
    Income,
    Expense,
    /// Giroconti, saldi iniziali, compensazioni: non sono costi né ricavi.
    Technical,
}

impl AccountKind {
    pub fn label(self) -> &'static str {
        match self {
            AccountKind::Resource => "Conto finanziario",
            AccountKind::Income => "Entrata",
            AccountKind::Expense => "Uscita",
            AccountKind::Technical => "Tecnico",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct LedgerAccount {
    pub code: String,
    /// Nome come compare in prima nota (conto o categoria del movimento).
    pub name: String,
    pub kind: AccountKind,
    pub group: String,
    /// Codice del conto nel programma dello studio.
    pub export_code: String,
    /// Conti bancari: IBAN, per riconoscere l'estratto conto.
    pub iban: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ChartOfAccounts {
    pub accounts: Vec<LedgerAccount>,
    /// Transcodifica IVA: chiave "22", "10", "N2.2"... -> codice IVA dello studio.
    pub vat_codes: Vec<(String, String)>,
    /// Conto dello studio per incassi da clienti e pagamenti a fornitori (movimenti delle
    /// scadenze, senza voce).
    pub customers_code: String,
    pub suppliers_code: String,
}

fn acc(code: &str, name: &str, kind: AccountKind, group: &str) -> LedgerAccount {
    LedgerAccount { code: code.into(), name: name.into(), kind, group: group.into(), ..Default::default() }
}

impl ChartOfAccounts {
    /// Piano proposto alla prima apertura: si adatta e si completa con i codici dello studio.
    pub fn standard() -> Self {
        use AccountKind::*;
        ChartOfAccounts {
            accounts: vec![
                acc("C01", "Cassa", Resource, "Disponibilità liquide"),
                acc("B01", "Banca", Resource, "Disponibilità liquide"),
                acc("R01", "Vendite", Income, "Ricavi"),
                acc("R02", "Altri ricavi", Income, "Ricavi"),
                acc("R03", "Interessi attivi", Income, "Proventi finanziari"),
                acc("R04", "Buoni regalo", Income, "Ricavi"),
                acc("S01", "Acquisti di merci", Expense, "Costi per acquisti"),
                acc("S02", "Affitto", Expense, "Costi per godimento beni di terzi"),
                acc("S03", "Utenze", Expense, "Costi per servizi"),
                acc("S04", "Telefono e internet", Expense, "Costi per servizi"),
                acc("S05", "Carburante", Expense, "Costi per servizi"),
                acc("S06", "Consulenze", Expense, "Costi per servizi"),
                acc("S07", "Spese bancarie", Expense, "Oneri finanziari"),
                acc("S08", "Stipendi", Expense, "Costi del personale"),
                acc("S09", "Contributi", Expense, "Costi del personale"),
                acc("S10", "Imposte", Expense, "Oneri tributari"),
                acc("S11", "Assicurazioni", Expense, "Costi per servizi"),
                acc("T01", TRANSFER, Technical, "Movimenti tecnici"),
                acc("T02", OPENING, Technical, "Movimenti tecnici"),
                acc("T03", crate::COMPENSATION_ACCOUNT, Technical, "Movimenti tecnici"),
            ],
            vat_codes: Vec::new(),
            customers_code: String::new(),
            suppliers_code: String::new(),
        }
    }

    pub fn find(&self, name: &str) -> Option<&LedgerAccount> {
        self.accounts.iter().find(|a| a.name.eq_ignore_ascii_case(name.trim()))
    }

    pub fn is_technical(&self, category: &str) -> bool {
        let c = category.trim();
        c.eq_ignore_ascii_case(TRANSFER) || c.eq_ignore_ascii_case(OPENING) || self.find(c).map(|a| a.kind == AccountKind::Technical).unwrap_or(false)
    }

    fn export_of(&self, name: &str) -> String {
        self.find(name).map(|a| if a.export_code.trim().is_empty() { a.code.clone() } else { a.export_code.clone() }).unwrap_or_default()
    }

    fn vat_export(&self, key: &str) -> String {
        self.vat_codes.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.clone()).unwrap_or_else(|| key.to_string())
    }
}

/// Totale per voce di entrata o di uscita.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CategoryTotal {
    pub category: String,
    pub group: String,
    pub income: Decimal,
    pub expense: Decimal,
}

/// Voce di un modello F24 (sezione Erario).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct F24Item {
    pub code: String,
    pub label: String,
    /// Rateazione / mese di riferimento (es. "0011" per novembre) e anno di riferimento.
    pub period: String,
    pub year: i32,
    pub amount: Decimal,
}

/// Versamento F24 registrato.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct F24Payment {
    pub id: i64,
    pub date: NaiveDate,
    pub account: String,
    pub total: Decimal,
    pub items: Vec<F24Item>,
}

/// Proposta di versamento con la sua scadenza.
#[derive(Debug, Clone, PartialEq)]
pub struct F24Proposal {
    pub due: NaiveDate,
    pub items: Vec<F24Item>,
}

/// Codice tributo delle ritenute d'acconto per causale di pagamento: provvigioni (Q, R) 1038,
/// lavoro autonomo e altri compensi 1040.
pub fn withholding_tax_code(reason: &str) -> &'static str {
    match reason.trim() {
        "Q" | "R" => "1038",
        _ => "1040",
    }
}

/// Giorno 16 del mese successivo (spostato al lunedì se cade di sabato o domenica).
pub fn sixteenth_after(d: NaiveDate) -> NaiveDate {
    let (y, m) = if d.month() == 12 { (d.year() + 1, 1) } else { (d.year(), d.month() + 1) };
    let mut due = NaiveDate::from_ymd_opt(y, m, 16).unwrap();
    while matches!(due.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun) {
        due = due.succ_opt().unwrap();
    }
    due
}

impl Store {
    pub fn chart_of_accounts(&self) -> Result<ChartOfAccounts> {
        let v: Option<ChartOfAccounts> = self.get_setting("chart_of_accounts")?;
        Ok(v.unwrap_or_else(ChartOfAccounts::standard))
    }

    pub fn set_chart_of_accounts(&self, c: &ChartOfAccounts) -> Result<()> {
        let mut codes = std::collections::HashSet::new();
        for a in &c.accounts {
            if a.name.trim().is_empty() {
                return Err(StoreError::Invalid("Ogni conto deve avere un nome".into()));
            }
            if !a.code.trim().is_empty() && !codes.insert(a.code.trim().to_uppercase()) {
                return Err(StoreError::Invalid(format!("Codice {} usato due volte nel piano dei conti", a.code.trim())));
            }
        }
        self.set_setting("chart_of_accounts", &Some(c.clone()))
    }

    fn add_linked_entry(&self, e: &CashEntry, link: &str) -> Result<i64> {
        let id = self.add_cash_entry(e)?;
        self.conn().execute("UPDATE cash_entries SET link = ?1 WHERE id = ?2", params![link, id])?;
        Ok(id)
    }

    /// Giroconto fra due conti finanziari (prelievo, versamento in banca, ricarica della
    /// carta): due movimenti collegati che si annullano, fuori da entrate e uscite.
    pub fn transfer(&self, from: &str, to: &str, date: NaiveDate, amount: Decimal, description: &str) -> Result<()> {
        if from.trim().is_empty() || to.trim().is_empty() || from.trim().eq_ignore_ascii_case(to.trim()) {
            return Err(StoreError::Invalid("Indicare due conti diversi".into()));
        }
        if amount <= Decimal::ZERO {
            return Err(StoreError::Invalid("L'importo del giroconto deve essere positivo".into()));
        }
        let tx = self.begin()?;
        let link = format!("giro:{}:{}", date_to_sql(date), chrono::Local::now().timestamp_nanos_opt().unwrap_or(0));
        let text = if description.trim().is_empty() { format!("Giroconto da {} a {}", from.trim(), to.trim()) } else { description.trim().to_string() };
        self.add_linked_entry(&CashEntry { date, description: text.clone(), account: from.trim().into(), amount: -amount, category: TRANSFER.into(), ..Default::default() }, &link)?;
        self.add_linked_entry(&CashEntry { date, description: text, account: to.trim().into(), amount, category: TRANSFER.into(), ..Default::default() }, &link)?;
        tx.commit()?;
        Ok(())
    }

    /// Elimina un movimento libero; per un giroconto anche il movimento gemello.
    pub fn delete_cash_entry_linked(&self, id: i64) -> Result<()> {
        let link: Option<String> = self.conn().query_row("SELECT link FROM cash_entries WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten();
        let tx = self.begin()?;
        match link.filter(|l| l.starts_with("giro:")) {
            Some(l) => {
                let ids: Vec<i64> = self.conn().prepare("SELECT id FROM cash_entries WHERE link = ?1")?.query_map([&l], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
                for i in ids {
                    self.delete_cash_entry(i)?;
                }
            }
            None => self.delete_cash_entry(id)?,
        }
        tx.commit()?;
        Ok(())
    }

    /// Saldo iniziale di un conto alla data (sostituisce quello già inserito).
    pub fn set_opening_balance(&self, account: &str, date: NaiveDate, amount: Decimal) -> Result<()> {
        if account.trim().is_empty() {
            return Err(StoreError::Invalid("Indicare il conto".into()));
        }
        let tx = self.begin()?;
        self.conn().execute("DELETE FROM cash_entries WHERE account = ?1 AND category = ?2 AND document_id IS NULL AND payment_id IS NULL", params![account.trim(), OPENING])?;
        if !amount.is_zero() {
            self.add_cash_entry(&CashEntry { date, description: format!("Saldo iniziale al {}", date.format("%d/%m/%Y")), account: account.trim().into(), amount, category: OPENING.into(), ..Default::default() })?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Saldo di un conto a fine giornata `at`.
    pub fn balance_at(&self, account: &str, at: NaiveDate) -> Result<Decimal> {
        Ok(self.cash_entries(None, Some(at), Some(account))?.iter().map(|e| e.amount).sum())
    }

    /// Entrate e uscite per voce nel periodo (senza giroconti, saldi iniziali e
    /// compensazioni). I movimenti delle scadenze senza voce vanno in "Incassi da clienti"
    /// e "Pagamenti a fornitori".
    pub fn category_totals(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<CategoryTotal>> {
        let chart = self.chart_of_accounts()?;
        let mut m: BTreeMap<String, CategoryTotal> = BTreeMap::new();
        for e in self.cash_entries(Some(from), Some(to), None)? {
            if e.account == crate::COMPENSATION_ACCOUNT || chart.is_technical(&e.category) {
                continue;
            }
            let cat = if !e.category.trim().is_empty() {
                e.category.trim().to_string()
            } else if e.due_date_id.is_some() || e.payment_id.is_some() || e.document_id.is_some() {
                if e.amount >= Decimal::ZERO { "Incassi da clienti".into() } else { "Pagamenti a fornitori".into() }
            } else {
                "Senza voce".into()
            };
            let t = m.entry(cat.to_lowercase()).or_insert_with(|| CategoryTotal { group: chart.find(&cat).map(|a| a.group.clone()).unwrap_or_default(), category: cat.clone(), ..Default::default() });
            if e.amount >= Decimal::ZERO {
                t.income += e.amount;
            } else {
                t.expense -= e.amount;
            }
        }
        Ok(m.into_values().collect())
    }

    /// Prima nota per lo studio, CSV con separatore ";": data, conto finanziario e voce con
    /// i codici transcodificati, descrizione, dare/avere, documento e partita IVA.
    pub fn ledger_export_csv(&self, from: NaiveDate, to: NaiveDate) -> Result<String> {
        let chart = self.chart_of_accounts()?;
        let mut out = String::from("Data;Conto;Codice conto;Voce;Codice voce;Descrizione;Entrata;Uscita;Documento;Partita IVA/C.F.\r\n");
        let mut parties: BTreeMap<i64, String> = BTreeMap::new();
        for e in self.cash_entries(Some(from), Some(to), None)? {
            let (voce, code) = if !e.category.trim().is_empty() {
                (e.category.trim().to_string(), chart.export_of(&e.category))
            } else if e.payment_id.is_some() || e.due_date_id.is_some() || e.document_id.is_some() {
                if e.amount >= Decimal::ZERO { ("Incassi da clienti".to_string(), chart.customers_code.clone()) } else { ("Pagamenti a fornitori".to_string(), chart.suppliers_code.clone()) }
            } else {
                (String::new(), String::new())
            };
            let doc = match e.document_id {
                Some(id) => self.document(id).map(|d| format!("{} {} del {}", d.kind.label(), d.number_label(), d.date.format("%d/%m/%Y"))).unwrap_or_default(),
                None => String::new(),
            };
            let vat = match e.party_id {
                Some(p) => parties.entry(p).or_insert_with(|| self.party(p).ok().flatten().map(|p| if p.vat_number.trim().is_empty() { p.tax_code } else { p.vat_number }).unwrap_or_default()).clone(),
                None => String::new(),
            };
            let amount = |d: Decimal| if d.is_zero() { String::new() } else { num2(d) };
            let row = [
                e.date.format("%d/%m/%Y").to_string(),
                e.account.clone(),
                chart.export_of(&e.account),
                voce,
                code,
                e.description.clone(),
                amount(e.amount.max(Decimal::ZERO)),
                amount((-e.amount).max(Decimal::ZERO)),
                doc,
                vat,
            ];
            out.push_str(&row.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(";"));
            out.push_str("\r\n");
        }
        Ok(out)
    }

    /// Registri IVA del periodo per lo studio, una riga per aliquota con il codice IVA
    /// transcodificato.
    pub fn vat_export_csv(&self, from: NaiveDate, to: NaiveDate) -> Result<String> {
        let chart = self.chart_of_accounts()?;
        let mut out = String::from("Registro;Data registrazione;Data documento;Numero;Numero fornitore;Tipo;Soggetto;Partita IVA/C.F.;Aliquota;Codice IVA;Imponibile;Imposta;Esigibilità\r\n");
        for (reg, name) in [(Register::Sales, "Vendite"), (Register::Purchases, "Acquisti")] {
            for r in self.vat_register(reg, from, to)? {
                for a in &r.rates {
                    let key = a.natura.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| a.rate.normalize().to_string());
                    let row = [
                        name.to_string(),
                        r.date.format("%d/%m/%Y").to_string(),
                        r.doc_date.format("%d/%m/%Y").to_string(),
                        r.number.clone(),
                        r.external_number.clone(),
                        format!("{} {}", r.kind_label, r.fe_type).trim().to_string(),
                        r.party.clone(),
                        r.party_vat.clone(),
                        a.label(),
                        chart.vat_export(&key),
                        num2(a.taxable),
                        num2(a.tax),
                        a.liability.clone(),
                    ];
                    out.push_str(&row.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(";"));
                    out.push_str("\r\n");
                }
            }
        }
        Ok(out)
    }

    /// Versamenti F24 da fare per i pagamenti del mese: ritenute operate sui compensi
    /// pagati (codice 1040 o 1038, entro il 16 del mese dopo) e IVA del periodo liquidato.
    pub fn f24_proposals(&self, year: i32, month: u32) -> Result<Vec<F24Proposal>> {
        let mut out = Vec::new();
        let first = NaiveDate::from_ymd_opt(year, month, 1).ok_or_else(|| StoreError::Invalid("Mese non valido".into()))?;
        let mut by_code: BTreeMap<&'static str, Decimal> = BTreeMap::new();
        for r in self.withholding_rows(year, true)? {
            let Some(paid) = r.paid_on else { continue };
            if paid.year() != year || paid.month() != month || !matches!(r.kind.as_str(), "RT01" | "RT02") {
                continue;
            }
            *by_code.entry(withholding_tax_code(&r.reason)).or_default() += r.amount;
        }
        let items: Vec<F24Item> = by_code
            .into_iter()
            .filter(|(_, a)| *a > Decimal::ZERO)
            .map(|(code, amount)| F24Item {
                code: code.into(),
                label: if code == "1038" { "Ritenute su provvigioni".into() } else { "Ritenute su redditi di lavoro autonomo".into() },
                period: format!("{:04}", month),
                year,
                amount: openfatt_core::money::round2(amount),
            })
            .collect();
        if !items.is_empty() {
            out.push(F24Proposal { due: sixteenth_after(first), items });
        }
        // IVA: periodo che si chiude con questo mese
        let s = self.vat_settings()?;
        let p = Period::containing(first, s.periodicity);
        if p.last().month() == month {
            let liq = self.vat_liquidations(p.year, p.index)?;
            if let Some((_, l)) = liq.last() {
                if l.to_pay > Decimal::ZERO {
                    out.push(F24Proposal {
                        due: p.payment_due(),
                        items: vec![F24Item { code: p.f24_code().into(), label: format!("IVA {}", p.label()), period: String::new(), year: p.year, amount: l.to_pay }],
                    });
                }
            }
        }
        Ok(out)
    }

    /// Registra un versamento F24: movimento di prima nota (voce "Imposte") e registro.
    pub fn record_f24(&self, date: NaiveDate, account: &str, items: &[F24Item]) -> Result<i64> {
        let total: Decimal = items.iter().map(|i| i.amount).sum();
        if items.is_empty() || total <= Decimal::ZERO {
            return Err(StoreError::Invalid("Il modello F24 non ha importi".into()));
        }
        let tx = self.begin()?;
        let codes: Vec<String> = items.iter().map(|i| i.code.clone()).collect();
        let entry = self.add_cash_entry(&CashEntry { date, description: format!("Versamento F24 - codici {}", codes.join(", ")), account: account.trim().into(), amount: -total, category: "Imposte".into(), ..Default::default() })?;
        self.conn().execute(
            "INSERT INTO f24_payments(date, account, total, items, cash_entry_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![date_to_sql(date), account.trim(), dec_to_sql(total), serde_json::to_string(items)?, entry],
        )?;
        let id = self.conn().last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    pub fn f24_payments(&self, year: i32) -> Result<Vec<F24Payment>> {
        let mut st = self.conn().prepare("SELECT id, date, account, total, items FROM f24_payments WHERE substr(date, 1, 4) = ?1 ORDER BY date, id")?;
        let rows = st.query_map([format!("{:04}", year)], |r| {
            Ok(F24Payment {
                id: r.get(0)?,
                date: crate::date_from_sql(&r.get::<_, String>(1)?),
                account: r.get(2)?,
                total: dec_from_sql(Some(r.get::<_, String>(3)?)),
                items: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Annulla un versamento F24 e il suo movimento di prima nota.
    pub fn delete_f24(&self, id: i64) -> Result<()> {
        let entry: Option<Option<i64>> = self.conn().query_row("SELECT cash_entry_id FROM f24_payments WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        let tx = self.begin()?;
        if let Some(Some(e)) = entry {
            self.delete_cash_entry(e)?;
        }
        self.conn().execute("DELETE FROM f24_payments WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }
}

/// Importo con due decimali e la virgola.
fn num2(d: Decimal) -> String {
    format!("{:.2}", d.round_dp(2)).replace('.', ",")
}

fn csv_cell(s: &str) -> String {
    // niente formule da fuori (=, +, -, @ in testa) e virgolette raddoppiate
    let s = if s.starts_with(['=', '+', '@']) || (s.starts_with('-') && s[1..].parse::<f64>().is_err()) { format!("'{}", s) } else { s.to_string() };
    if s.contains([';', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

// ----------------------------------------------------------------------------
// Riconciliazione bancaria
// ----------------------------------------------------------------------------

/// Esito proposto per una riga dell'estratto conto.
#[derive(Debug, Clone, PartialEq)]
pub enum ReconMatch {
    /// Già in prima nota (stesso conto, importo e data vicina, o riga già importata).
    Recorded(i64),
    /// Incasso o pagamento di scadenze aperte (stesso importo, nome o numero nella causale).
    Dues(Vec<i64>),
    /// Da registrare come movimento libero con la voce indicata.
    New(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReconLine {
    pub line: openfatt_bank::camt::StatementLine,
    pub matched: ReconMatch,
    /// Descrizione della proposta (cliente, documento...).
    pub note: String,
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3).map(|w| w.to_string()).collect()
}

const STOP: [&str; 12] = ["srl", "spa", "snc", "sas", "s.r.l", "della", "delle", "fattura", "fatt", "pagamento", "bonifico", "saldo"];

impl Store {
    /// Confronta le righe dell'estratto conto con la prima nota del conto e con le
    /// scadenze aperte.
    pub fn reconcile(&self, account: &str, lines: &[openfatt_bank::camt::StatementLine]) -> Result<Vec<ReconLine>> {
        let mut out = Vec::new();
        let mut used_entries: std::collections::HashSet<i64> = Default::default();
        let mut used_dues: std::collections::HashSet<i64> = Default::default();
        let all_dues = self.due_list(true, None)?;
        let chart = self.chart_of_accounts()?;
        for l in lines {
            let link = format!("camt:{}", l.key());
            let known: Option<i64> = self.conn().query_row("SELECT id FROM cash_entries WHERE link = ?1", [&link], |r| r.get(0)).optional()?;
            if let Some(id) = known {
                out.push(ReconLine { line: l.clone(), matched: ReconMatch::Recorded(id), note: "già importata".into() });
                continue;
            }
            // stesso conto, stesso importo, entro tre giorni (non già abbinato a un'altra riga)
            let linked: std::collections::HashSet<i64> = {
                let mut st = self.conn().prepare_cached("SELECT id FROM cash_entries WHERE account = ?1 AND link LIKE 'camt:%' AND date BETWEEN ?2 AND ?3")?;
                let rows = st.query_map(params![account, date_to_sql(l.date - chrono::Duration::days(3)), date_to_sql(l.date + chrono::Duration::days(3))], |r| r.get(0))?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            let near = self
                .cash_entries(Some(l.date - chrono::Duration::days(3)), Some(l.date + chrono::Duration::days(3)), Some(account))?
                .into_iter()
                .filter(|e| e.amount == l.amount && !used_entries.contains(&e.id) && !linked.contains(&e.id))
                .min_by_key(|e| (e.date - l.date).num_days().abs());
            if let Some(e) = near {
                used_entries.insert(e.id);
                out.push(ReconLine { line: l.clone(), matched: ReconMatch::Recorded(e.id), note: e.description.clone() });
                continue;
            }
            // scadenze aperte con lo stesso importo: per nome del soggetto o numero del documento
            let receivable = l.amount > Decimal::ZERO;
            let text = format!("{} {}", l.counterparty, l.remittance);
            let tw = words(&text);
            let mut best: Option<(i32, &crate::DueRow)> = None;
            for d in all_dues.iter().filter(|d| d.customer_side() == receivable && !used_dues.contains(&d.id) && !d.draft) {
                if d.signed_residual().abs() != l.amount.abs() {
                    continue;
                }
                let mut score = 1;
                let name_words = words(&d.party_name);
                if name_words.iter().filter(|w| !STOP.contains(&w.as_str())).any(|w| tw.contains(w)) {
                    score += 2;
                }
                let num = d.doc_label.split_whitespace().find(|t| t.chars().any(|c| c.is_ascii_digit())).unwrap_or_default().trim_matches(|c: char| !c.is_ascii_alphanumeric());
                if !num.is_empty() && tw.iter().any(|w| w == &num.to_lowercase()) {
                    score += 2;
                }
                if best.map(|(s, _)| score > s).unwrap_or(true) {
                    best = Some((score, d));
                }
            }
            match best.filter(|(s, _)| *s >= 3) {
                Some((_, d)) => {
                    used_dues.insert(d.id);
                    out.push(ReconLine { line: l.clone(), matched: ReconMatch::Dues(vec![d.id]), note: format!("{} · {}", d.party_name, d.doc_label) });
                }
                None => {
                    let guess = if l.amount < Decimal::ZERO && l.remittance.to_lowercase().contains("comm") || l.remittance.to_lowercase().contains("spese") { "Spese bancarie" } else { "" };
                    let guess = if chart.find(guess).is_some() { guess.to_string() } else { String::new() };
                    out.push(ReconLine { line: l.clone(), matched: ReconMatch::New(guess), note: String::new() });
                }
            }
        }
        Ok(out)
    }

    /// Registra le righe confermate: incassi e pagamenti sulle scadenze, movimenti liberi con
    /// la voce scelta. Le righe già registrate si saltano. Restituisce quante ne ha scritte.
    pub fn apply_reconciliation(&self, account: &str, lines: &[ReconLine]) -> Result<usize> {
        let tx = self.begin()?;
        let mut n = 0;
        for r in lines {
            let link = format!("camt:{}", r.line.key());
            match &r.matched {
                ReconMatch::Recorded(_) => continue,
                ReconMatch::Dues(ids) => {
                    let dues = self.dues_by_id(ids)?;
                    let mut left = r.line.amount.abs();
                    let mut allocations = Vec::new();
                    for d in &dues {
                        let a = d.residual().min(left);
                        if a > Decimal::ZERO {
                            allocations.push(Allocation { due_id: d.id, amount: a, writeoff: false });
                            left -= a;
                        }
                    }
                    let pid = self.register_payment(&NewPayment {
                        date: r.line.date,
                        mp_code: "MP05".into(),
                        account: account.trim().into(),
                        reference: r.line.reference.clone(),
                        notes: r.line.remittance.chars().take(200).collect(),
                        allocations,
                    })?;
                    self.conn().execute("UPDATE cash_entries SET link = ?1 WHERE payment_id = ?2", params![link, pid])?;
                }
                ReconMatch::New(cat) => {
                    let desc = [r.line.counterparty.trim(), r.line.remittance.trim()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" - ");
                    self.add_linked_entry(&CashEntry { date: r.line.date, description: if desc.is_empty() { "Movimento bancario".into() } else { desc.chars().take(250).collect() }, account: account.trim().into(), amount: r.line.amount, category: cat.clone(), ..Default::default() }, &link)?;
                }
            }
            n += 1;
        }
        tx.commit()?;
        Ok(n)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;
    use openfatt_core::model::*;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }

    #[test]
    fn giroconti_e_saldi_iniziali_fuori_da_entrate_e_uscite() {
        let s = Store::open_in_memory().unwrap();
        s.set_opening_balance("Cassa", d(2026, 1, 1), dec!(300)).unwrap();
        s.set_opening_balance("Banca", d(2026, 1, 1), dec!(5000)).unwrap();
        // il saldo iniziale si corregge, non si somma
        s.set_opening_balance("Banca", d(2026, 1, 1), dec!(4800)).unwrap();
        s.add_cash_entry(&CashEntry { date: d(2026, 1, 5), description: "Affitto gennaio".into(), account: "Banca".into(), amount: dec!(-800), category: "Affitto".into(), ..Default::default() }).unwrap();
        s.transfer("Cassa", "Banca", d(2026, 1, 10), dec!(250), "Versamento contanti").unwrap();
        assert_eq!(s.balance_at("Cassa", d(2026, 1, 31)).unwrap(), dec!(50));
        assert_eq!(s.balance_at("Banca", d(2026, 1, 31)).unwrap(), dec!(4250));
        assert_eq!(s.balance_at("Banca", d(2026, 1, 7)).unwrap(), dec!(4000));
        let tot = s.category_totals(d(2026, 1, 1), d(2026, 12, 31)).unwrap();
        assert_eq!(tot.len(), 1, "{:?}", tot);
        assert_eq!(tot[0].category, "Affitto");
        assert_eq!(tot[0].expense, dec!(800));
        assert_eq!(tot[0].group, "Costi per godimento beni di terzi");
        // eliminando un lato del giroconto sparisce anche l'altro
        let giro = s.cash_entries(None, None, Some("Cassa")).unwrap().into_iter().find(|e| e.category == TRANSFER).unwrap();
        s.delete_cash_entry_linked(giro.id).unwrap();
        assert_eq!(s.balance_at("Banca", d(2026, 1, 31)).unwrap(), dec!(4000));
        assert_eq!(s.balance_at("Cassa", d(2026, 1, 31)).unwrap(), dec!(300));
        assert!(s.transfer("Cassa", "cassa", d(2026, 1, 10), dec!(1), "").is_err());
        assert!(s.transfer("Cassa", "Banca", d(2026, 1, 10), dec!(0), "").is_err());
    }

    #[test]
    fn piano_dei_conti_e_transcodifica_per_lo_studio() {
        let s = Store::open_in_memory().unwrap();
        let mut c = s.chart_of_accounts().unwrap();
        assert!(c.find("Cassa").is_some());
        c.accounts.iter_mut().find(|a| a.name == "Banca").unwrap().export_code = "18.01.002".into();
        c.accounts.iter_mut().find(|a| a.name == "Utenze").unwrap().export_code = "66.10.05".into();
        c.vat_codes.push(("22".into(), "V22".into()));
        c.customers_code = "14.01".into();
        s.set_chart_of_accounts(&c).unwrap();
        let mut dup = c.clone();
        dup.accounts.push(LedgerAccount { code: "C01".into(), name: "Altra cassa".into(), ..Default::default() });
        assert!(s.set_chart_of_accounts(&dup).is_err());
        s.add_cash_entry(&CashEntry { date: d(2026, 2, 3), description: "Bolletta; luce \"febbraio\"".into(), account: "Banca".into(), amount: dec!(-120.5), category: "Utenze".into(), ..Default::default() }).unwrap();
        let csv = s.ledger_export_csv(d(2026, 1, 1), d(2026, 12, 31)).unwrap();
        let row = csv.lines().nth(1).unwrap();
        assert!(row.starts_with("03/02/2026;Banca;18.01.002;Utenze;66.10.05;\"Bolletta; luce \"\"febbraio\"\"\";;120,50;"), "{}", row);
        // registri IVA con il codice dello studio
        let v = VatRef::from_code(&s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap());
        let p = s.save_party(&Party { name: "=SOMMA(A1)".into(), vat_number: "12345678903".into(), ..Default::default() }).unwrap();
        let mut doc = Document::new(DocKind::Invoice);
        doc.date = d(2026, 2, 4);
        doc.party_id = Some(p.id);
        doc.lines.push(DocLine { description: "Merce".into(), qty: dec!(1), unit_price: dec!(100), vat: v, ..Default::default() });
        s.issue_document(s.save_document(&doc).unwrap().id).unwrap();
        let vat = s.vat_export_csv(d(2026, 1, 1), d(2026, 12, 31)).unwrap();
        let row = vat.lines().nth(1).unwrap();
        assert!(row.contains(";V22;100,00;22,00;I"), "{}", row);
        // le formule nei nomi arrivati da fuori non passano
        assert!(row.contains(";'=SOMMA(A1);"), "{}", row);
    }

    #[test]
    fn riconciliazione_da_estratto_conto() {
        let s = Store::open_in_memory().unwrap();
        let v = VatRef::from_code(&s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap());
        let rossi = s.save_party(&Party { name: "Rossi Arredamenti Srl".into(), vat_number: "12345678903".into(), ..Default::default() }).unwrap();
        let bianchi = s.save_party(&Party { name: "Bianchi Snc".into(), vat_number: "01234567890".into(), ..Default::default() }).unwrap();
        let mk = |p: &Party, amount: Decimal, date: NaiveDate| {
            let mut doc = Document::new(DocKind::Invoice);
            doc.date = date;
            doc.party_id = Some(p.id);
            doc.lines.push(DocLine { description: "Merce".into(), qty: dec!(1), unit_price: amount, vat: v.clone(), ..Default::default() });
            s.issue_document(s.save_document(&doc).unwrap().id).unwrap()
        };
        let f1 = mk(&rossi, dec!(500), d(2026, 10, 15));
        // stesso importo, altro cliente: non deve essere scelta
        let _f2 = mk(&bianchi, dec!(500), d(2026, 10, 16));
        // movimento già registrato a mano
        s.add_cash_entry(&CashEntry { date: d(2026, 11, 19), description: "Bolletta".into(), account: "Banca".into(), amount: dec!(-6), category: "Utenze".into(), ..Default::default() }).unwrap();
        let line = |date: NaiveDate, amount: Decimal, who: &str, text: &str, r: &str| openfatt_bank::camt::StatementLine { date, amount, counterparty: who.into(), remittance: text.into(), reference: r.into(), currency: "EUR".into(), value_date: None };
        let lines = vec![
            line(d(2026, 11, 10), dec!(610), "Rossi Arredamenti Srl", &format!("Saldo fattura n. {} del 15/10/2026", f1.number_label()), "E2E-1"),
            line(d(2026, 11, 20), dec!(-6), "Telefonia Spa", "Addebito SDD bolletta", ""),
            line(d(2026, 11, 30), dec!(-2.5), "", "Commissioni e spese", "BANK-9"),
            line(d(2026, 11, 30), dec!(99), "Sconosciuto", "Accredito", "X"),
        ];
        let r = s.reconcile("Banca", &lines).unwrap();
        let rossi_due = s.dues_of_document(f1.id).unwrap()[0].id;
        assert_eq!(r[0].matched, ReconMatch::Dues(vec![rossi_due]), "{:?}", r[0]);
        assert!(matches!(r[1].matched, ReconMatch::Recorded(_)));
        assert_eq!(r[2].matched, ReconMatch::New("Spese bancarie".into()));
        assert_eq!(r[3].matched, ReconMatch::New(String::new()));
        let n = s.apply_reconciliation("Banca", &r).unwrap();
        assert_eq!(n, 3);
        // fattura di Rossi incassata, quella di Bianchi no
        assert!(s.dues_of_document(f1.id).unwrap()[0].residual().is_zero());
        assert_eq!(s.balance_at("Banca", d(2026, 12, 31)).unwrap(), dec!(610) - dec!(6) - dec!(2.5) + dec!(99));
        // reimportando lo stesso estratto non si registra nulla due volte
        let again = s.reconcile("Banca", &lines).unwrap();
        assert!(again.iter().all(|x| matches!(x.matched, ReconMatch::Recorded(_))), "{:?}", again);
        assert_eq!(s.apply_reconciliation("Banca", &again).unwrap(), 0);
    }

    #[test]
    fn f24_ritenute_del_mese_e_registro() {
        let s = Store::open_in_memory().unwrap();
        let v = VatRef::from_code(&s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap());
        let prof = s.save_party(&Party { role: PartyRole::Supplier, name: "Avv. Verdi".into(), tax_code: "VRDLGU70A01L378X".into(), ..Default::default() }).unwrap();
        let agent = s.save_party(&Party { role: PartyRole::Supplier, name: "Agente Neri".into(), tax_code: "NRENRE70A01L378X".into(), ..Default::default() }).unwrap();
        let mk = |p: &Party, amount: Decimal, w: Withholding| {
            let mut doc = Document::new(DocKind::PurchaseInvoice);
            doc.date = d(2026, 10, 5);
            doc.external_number = "7".into();
            doc.party_id = Some(p.id);
            doc.lines.push(DocLine { description: "Compenso".into(), qty: dec!(1), unit_price: amount, vat: v.clone(), withholding: true, ..Default::default() });
            doc.withholdings.push(w);
            let doc = s.issue_document(s.save_document(&doc).unwrap().id).unwrap();
            let dues = s.dues_of_document(doc.id).unwrap();
            s.register_payment(&NewPayment { date: d(2026, 10, 28), mp_code: "MP05".into(), account: "Banca".into(), reference: String::new(), notes: String::new(), allocations: dues.iter().map(|x| Allocation { due_id: x.id, amount: x.residual(), writeoff: false }).collect() }).unwrap();
        };
        mk(&prof, dec!(1000), Withholding::default());
        mk(&agent, dec!(2000), Withholding { kind: "RT01".into(), rate: dec!(23), base_percent: dec!(50), payment_reason: "Q".into(), max_base: None });
        let p = s.f24_proposals(2026, 10).unwrap();
        let w = &p[0];
        // ottobre: versamento entro lunedì 16 novembre 2026
        assert_eq!(w.due, d(2026, 11, 16));
        let get = |c: &str| w.items.iter().find(|i| i.code == c).map(|i| i.amount);
        assert_eq!(get("1040"), Some(dec!(200.00)));
        assert_eq!(get("1038"), Some(dec!(230.00)));
        assert_eq!(w.items[0].period, "0010");
        assert!(s.f24_proposals(2026, 9).unwrap().is_empty());
        let id = s.record_f24(d(2026, 11, 16), "Banca", &w.items).unwrap();
        let reg = s.f24_payments(2026).unwrap();
        assert_eq!(reg.len(), 1);
        assert_eq!(reg[0].total, dec!(430.00));
        let bal = s.balance_at("Banca", d(2026, 12, 31)).unwrap();
        s.delete_f24(id).unwrap();
        assert_eq!(s.balance_at("Banca", d(2026, 12, 31)).unwrap(), bal + dec!(430));
        assert!(s.f24_payments(2026).unwrap().is_empty());
        // scadenza al 16 spostata dal fine settimana
        assert_eq!(sixteenth_after(d(2026, 7, 3)), d(2026, 8, 17));
        assert_eq!(sixteenth_after(d(2026, 12, 3)), d(2027, 1, 18));
    }
}
