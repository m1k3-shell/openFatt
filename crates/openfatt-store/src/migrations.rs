//! Migrazioni dello schema. Ogni versione si applica una sola volta, in transazione.

use rusqlite::Connection;

pub(crate) const MIGRATIONS: &[&str] = &[
    // 1 - schema iniziale
    r#"
    CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE counters(name TEXT PRIMARY KEY, value INTEGER NOT NULL);
    CREATE TABLE audit(
        id INTEGER PRIMARY KEY, ts TEXT NOT NULL, entity TEXT NOT NULL, entity_id INTEGER,
        action TEXT NOT NULL, details TEXT
    );
    CREATE TABLE vat_codes(id INTEGER PRIMARY KEY, code TEXT NOT NULL UNIQUE, data TEXT NOT NULL);
    CREATE TABLE payment_methods(id INTEGER PRIMARY KEY, name TEXT NOT NULL, data TEXT NOT NULL);

    CREATE TABLE parties(
        id INTEGER PRIMARY KEY,
        code TEXT,
        role TEXT NOT NULL,
        name TEXT NOT NULL,
        vat_number TEXT,
        tax_code TEXT,
        city TEXT,
        email TEXT,
        active INTEGER NOT NULL DEFAULT 1,
        version INTEGER NOT NULL DEFAULT 1,
        data TEXT NOT NULL,
        updated_at TEXT
    );
    CREATE UNIQUE INDEX ux_parties_code ON parties(code) WHERE code IS NOT NULL AND code <> '';
    CREATE INDEX ix_parties_vat ON parties(vat_number);
    CREATE INDEX ix_parties_name ON parties(name);

    CREATE TABLE products(
        id INTEGER PRIMARY KEY,
        code TEXT NOT NULL UNIQUE,
        description TEXT NOT NULL,
        barcode TEXT,
        category TEXT,
        active INTEGER NOT NULL DEFAULT 1,
        version INTEGER NOT NULL DEFAULT 1,
        data TEXT NOT NULL,
        updated_at TEXT
    );
    CREATE INDEX ix_products_barcode ON products(barcode);

    CREATE TABLE documents(
        id INTEGER PRIMARY KEY,
        kind TEXT NOT NULL,
        status TEXT NOT NULL,
        register TEXT NOT NULL,
        series TEXT NOT NULL DEFAULT '',
        year INTEGER NOT NULL,
        number INTEGER,
        date TEXT NOT NULL,
        party_id INTEGER REFERENCES parties(id),
        party_name TEXT,
        external_number TEXT,
        total TEXT,
        net_to_pay TEXT,
        version INTEGER NOT NULL DEFAULT 1,
        data TEXT NOT NULL,
        created_at TEXT,
        updated_at TEXT
    );
    CREATE UNIQUE INDEX ux_documents_number ON documents(register, series, year, number) WHERE number IS NOT NULL;
    CREATE INDEX ix_documents_kind ON documents(kind, year);
    CREATE INDEX ix_documents_party ON documents(party_id);

    CREATE TABLE doc_lines(
        id INTEGER PRIMARY KEY,
        document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        product_id INTEGER,
        qty TEXT NOT NULL,
        source_line_id INTEGER,
        source_doc_id INTEGER
    );
    CREATE INDEX ix_doc_lines_doc ON doc_lines(document_id);
    CREATE INDEX ix_doc_lines_source ON doc_lines(source_line_id);
    CREATE INDEX ix_doc_lines_source_doc ON doc_lines(source_doc_id);

    CREATE TABLE due_dates(
        id INTEGER PRIMARY KEY,
        document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        date TEXT NOT NULL,
        amount TEXT NOT NULL,
        mp_code TEXT,
        paid TEXT NOT NULL DEFAULT '0',
        writeoff TEXT NOT NULL DEFAULT '0',
        paid_date TEXT
    );
    CREATE INDEX ix_due_dates_doc ON due_dates(document_id);

    CREATE TABLE cash_entries(
        id INTEGER PRIMARY KEY,
        date TEXT NOT NULL,
        description TEXT,
        account TEXT NOT NULL,
        amount TEXT NOT NULL,
        document_id INTEGER REFERENCES documents(id) ON DELETE SET NULL,
        due_date_id INTEGER REFERENCES due_dates(id) ON DELETE SET NULL,
        party_id INTEGER,
        category TEXT
    );
    CREATE INDEX ix_cash_date ON cash_entries(date);

    CREATE TABLE stock_moves(
        id INTEGER PRIMARY KEY,
        date TEXT NOT NULL,
        product_id INTEGER NOT NULL REFERENCES products(id),
        qty TEXT NOT NULL,
        warehouse TEXT NOT NULL DEFAULT 'Principale',
        unit_value TEXT,
        reason TEXT,
        document_id INTEGER REFERENCES documents(id) ON DELETE CASCADE,
        line_id INTEGER,
        lot TEXT
    );
    CREATE INDEX ix_stock_product ON stock_moves(product_id);

    CREATE TABLE einvoices(
        id INTEGER PRIMARY KEY,
        direction TEXT NOT NULL,
        document_id INTEGER REFERENCES documents(id) ON DELETE SET NULL,
        file_name TEXT NOT NULL,
        sdi_id TEXT,
        status TEXT NOT NULL,
        xml BLOB NOT NULL,
        original BLOB,
        hash TEXT NOT NULL,
        party_name TEXT,
        party_vat TEXT,
        doc_type TEXT,
        doc_number TEXT,
        doc_date TEXT,
        total TEXT,
        created_at TEXT NOT NULL,
        sent_at TEXT,
        updated_at TEXT,
        last_message TEXT
    );
    CREATE UNIQUE INDEX ux_einvoices_file ON einvoices(direction, file_name);
    CREATE INDEX ix_einvoices_doc ON einvoices(document_id);

    CREATE TABLE einvoice_events(
        id INTEGER PRIMARY KEY,
        einvoice_id INTEGER REFERENCES einvoices(id) ON DELETE CASCADE,
        file_name TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL,
        sdi_id TEXT,
        received_at TEXT NOT NULL,
        description TEXT,
        raw BLOB
    );

    CREATE TABLE mail_seen(message_id TEXT PRIMARY KEY, seen_at TEXT NOT NULL, outcome TEXT);
    "#,
    // 2 - varianti, lotti con scadenza, numeri di serie
    r#"
    ALTER TABLE stock_moves ADD COLUMN variant TEXT NOT NULL DEFAULT '';
    ALTER TABLE stock_moves ADD COLUMN expiry TEXT;
    ALTER TABLE stock_moves ADD COLUMN serial TEXT NOT NULL DEFAULT '';
    CREATE INDEX ix_stock_trace ON stock_moves(product_id, variant, lot, serial);
    CREATE INDEX ix_stock_lot ON stock_moves(lot);
    CREATE INDEX ix_stock_serial ON stock_moves(serial);
    CREATE TABLE product_barcodes(
        barcode TEXT PRIMARY KEY,
        product_id INTEGER NOT NULL REFERENCES products(id) ON DELETE CASCADE,
        variant TEXT NOT NULL DEFAULT ''
    );
    "#,
    // 3 - listini e condizioni particolari dei soggetti
    r#"
    CREATE TABLE price_lists(id INTEGER PRIMARY KEY, name TEXT NOT NULL, data TEXT NOT NULL);
    CREATE TABLE price_list_items(
        list_id INTEGER NOT NULL REFERENCES price_lists(id) ON DELETE CASCADE,
        product_id INTEGER NOT NULL REFERENCES products(id) ON DELETE CASCADE,
        min_qty TEXT NOT NULL DEFAULT '0',
        price TEXT NOT NULL,
        PRIMARY KEY(list_id, product_id, min_qty)
    );
    CREATE TABLE party_prices(
        id INTEGER PRIMARY KEY,
        party_id INTEGER NOT NULL REFERENCES parties(id) ON DELETE CASCADE,
        product_id INTEGER REFERENCES products(id) ON DELETE CASCADE,
        category TEXT,
        data TEXT NOT NULL
    );
    CREATE INDEX ix_party_prices ON party_prices(party_id);
    "#,
    // 4 - magazzini
    r#"
    CREATE TABLE warehouses(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, data TEXT NOT NULL);
    CREATE INDEX ix_stock_warehouse ON stock_moves(product_id, warehouse);
    "#,
    // 5 - inventario fisico
    r#"
    CREATE TABLE inventory_sessions(
        id INTEGER PRIMARY KEY,
        number INTEGER NOT NULL,
        date TEXT NOT NULL,
        warehouse TEXT NOT NULL,
        description TEXT,
        status TEXT NOT NULL,
        created_at TEXT NOT NULL,
        closed_at TEXT
    );
    CREATE TABLE inventory_lines(
        id INTEGER PRIMARY KEY,
        session_id INTEGER NOT NULL REFERENCES inventory_sessions(id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        product_id INTEGER NOT NULL REFERENCES products(id),
        variant TEXT NOT NULL DEFAULT '',
        lot TEXT NOT NULL DEFAULT '',
        expiry TEXT,
        serial TEXT NOT NULL DEFAULT '',
        expected TEXT NOT NULL,
        counted TEXT,
        note TEXT
    );
    CREATE INDEX ix_inventory_lines ON inventory_lines(session_id);
    ALTER TABLE stock_moves ADD COLUMN inventory_id INTEGER;
    "#,
    // 6 - registro dei messaggi email (documenti, solleciti, comunicazioni)
    r#"
    CREATE TABLE mail_log(
        id INTEGER PRIMARY KEY,
        ts TEXT NOT NULL,
        kind TEXT NOT NULL,
        party_id INTEGER,
        document_id INTEGER,
        level INTEGER NOT NULL DEFAULT 0,
        recipients TEXT NOT NULL,
        subject TEXT NOT NULL,
        status TEXT NOT NULL,
        detail TEXT,
        dues TEXT
    );
    CREATE INDEX ix_mail_log_party ON mail_log(party_id, kind);
    CREATE INDEX ix_mail_log_doc ON mail_log(document_id);
    "#,
    // 7 - distinte bancarie (Ri.Ba., addebiti SDD, bonifici)
    r#"
    CREATE TABLE bank_batches(
        id INTEGER PRIMARY KEY,
        kind TEXT NOT NULL,
        year INTEGER NOT NULL,
        number INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        date TEXT NOT NULL,
        file_name TEXT NOT NULL,
        content TEXT NOT NULL,
        total TEXT NOT NULL,
        count INTEGER NOT NULL,
        status TEXT NOT NULL,
        note TEXT
    );
    CREATE UNIQUE INDEX ux_bank_batches ON bank_batches(kind, year, number);
    CREATE TABLE bank_batch_items(
        id INTEGER PRIMARY KEY,
        batch_id INTEGER NOT NULL REFERENCES bank_batches(id) ON DELETE CASCADE,
        due_id INTEGER NOT NULL REFERENCES due_dates(id),
        amount TEXT NOT NULL,
        ref TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT ''
    );
    CREATE INDEX ix_bank_items_due ON bank_batch_items(due_id);
    CREATE INDEX ix_bank_items_batch ON bank_batch_items(batch_id);
    "#,
    // 8 - documenti ricorrenti, allegati e foto, agenti, agenda, utenti e postazioni,
    //     carte fedeltà e buoni, negozi online, chiusure d'anno, conservazione
    r#"
    CREATE TABLE recurring(
        id INTEGER PRIMARY KEY,
        template_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        next_date TEXT NOT NULL,
        active INTEGER NOT NULL DEFAULT 1,
        data TEXT NOT NULL
    );
    CREATE TABLE attachments(
        id INTEGER PRIMARY KEY,
        entity TEXT NOT NULL,
        entity_id INTEGER NOT NULL,
        name TEXT NOT NULL,
        mime TEXT NOT NULL DEFAULT '',
        size INTEGER NOT NULL,
        content BLOB NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE INDEX ix_attachments ON attachments(entity, entity_id);
    CREATE TABLE agents(id INTEGER PRIMARY KEY, name TEXT NOT NULL, active INTEGER NOT NULL DEFAULT 1, data TEXT NOT NULL);
    CREATE TABLE commission_settlements(
        id INTEGER PRIMARY KEY,
        agent_id INTEGER NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
        date_from TEXT NOT NULL,
        date_to TEXT NOT NULL,
        created_at TEXT NOT NULL,
        total TEXT NOT NULL,
        data TEXT NOT NULL
    );
    CREATE TABLE agenda(
        id INTEGER PRIMARY KEY,
        date TEXT NOT NULL,
        time TEXT NOT NULL DEFAULT '',
        done INTEGER NOT NULL DEFAULT 0,
        party_id INTEGER,
        data TEXT NOT NULL
    );
    CREATE INDEX ix_agenda_date ON agenda(date);
    CREATE TABLE users(id INTEGER PRIMARY KEY, username TEXT NOT NULL UNIQUE COLLATE NOCASE, active INTEGER NOT NULL DEFAULT 1, data TEXT NOT NULL);
    CREATE TABLE sessions(
        id INTEGER PRIMARY KEY,
        username TEXT NOT NULL,
        host TEXT NOT NULL,
        started_at TEXT NOT NULL,
        seen_at TEXT NOT NULL
    );
    CREATE TABLE edit_locks(
        entity TEXT NOT NULL,
        entity_id INTEGER NOT NULL,
        session_id INTEGER NOT NULL,
        since TEXT NOT NULL,
        PRIMARY KEY(entity, entity_id)
    );
    CREATE TABLE cards(
        id INTEGER PRIMARY KEY,
        code TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL,
        party_id INTEGER,
        active INTEGER NOT NULL DEFAULT 1,
        data TEXT NOT NULL
    );
    CREATE TABLE card_moves(
        id INTEGER PRIMARY KEY,
        card_id INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
        date TEXT NOT NULL,
        points TEXT NOT NULL DEFAULT '0',
        amount TEXT NOT NULL DEFAULT '0',
        document_id INTEGER,
        note TEXT NOT NULL DEFAULT ''
    );
    CREATE INDEX ix_card_moves ON card_moves(card_id);
    CREATE TABLE shop_links(
        shop TEXT NOT NULL,
        entity TEXT NOT NULL,
        remote_id TEXT NOT NULL,
        local_id INTEGER NOT NULL,
        PRIMARY KEY(shop, entity, remote_id)
    );
    CREATE TABLE year_closures(year INTEGER PRIMARY KEY, closed_at TEXT NOT NULL, data TEXT NOT NULL);
    CREATE TABLE conservation(
        id INTEGER PRIMARY KEY,
        year INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        file_name TEXT NOT NULL,
        count INTEGER NOT NULL,
        hash TEXT NOT NULL,
        status TEXT NOT NULL,
        note TEXT NOT NULL DEFAULT ''
    );
    ALTER TABLE documents ADD COLUMN agent_id INTEGER;
    ALTER TABLE audit ADD COLUMN user TEXT NOT NULL DEFAULT '';
    CREATE INDEX ix_documents_agent ON documents(agent_id);
    CREATE INDEX ix_documents_date ON documents(date);
    CREATE INDEX ix_doc_lines_product ON doc_lines(product_id);
    CREATE INDEX ix_products_code_nocase ON products(code COLLATE NOCASE);
    CREATE INDEX ix_parties_role ON parties(role, active);
    "#,
    r#"
    INSERT OR IGNORE INTO vat_codes(code, data) SELECT '22sp', '{"id":0,"code":"22sp","description":"Imponibile 22% con scissione dei pagamenti","rate":"22","natura":null,"legal_ref":"Scissione dei pagamenti art. 17-ter DPR 633/72","active":true,"split_payment":true}' WHERE EXISTS (SELECT 1 FROM vat_codes);
    INSERT OR IGNORE INTO vat_codes(code, data) SELECT '10sp', '{"id":0,"code":"10sp","description":"Imponibile 10% con scissione dei pagamenti","rate":"10","natura":null,"legal_ref":"Scissione dei pagamenti art. 17-ter DPR 633/72","active":true,"split_payment":true}' WHERE EXISTS (SELECT 1 FROM vat_codes);
    INSERT OR IGNORE INTO vat_codes(code, data) SELECT '5sp', '{"id":0,"code":"5sp","description":"Imponibile 5% con scissione dei pagamenti","rate":"5","natura":null,"legal_ref":"Scissione dei pagamenti art. 17-ter DPR 633/72","active":true,"split_payment":true}' WHERE EXISTS (SELECT 1 FROM vat_codes);
    INSERT OR IGNORE INTO vat_codes(code, data) SELECT '4sp', '{"id":0,"code":"4sp","description":"Imponibile 4% con scissione dei pagamenti","rate":"4","natura":null,"legal_ref":"Scissione dei pagamenti art. 17-ter DPR 633/72","active":true,"split_payment":true}' WHERE EXISTS (SELECT 1 FROM vat_codes);
    CREATE INDEX IF NOT EXISTS ix_doc_lines_source ON doc_lines(source_doc_id);
    "#,    r#"
    -- dati di riepilogo dei documenti in colonne proprie: gli elenchi non devono più
    -- leggere tutto il documento salvato (alcuni KB l'uno) per mostrarne pochi campi
    ALTER TABLE documents ADD COLUMN s_legacy_fe INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE documents ADD COLUMN s_order_status TEXT NOT NULL DEFAULT '';
    ALTER TABLE documents ADD COLUMN s_expected_date TEXT;
    ALTER TABLE documents ADD COLUMN s_notes TEXT NOT NULL DEFAULT '';
    ALTER TABLE documents ADD COLUMN s_city TEXT NOT NULL DEFAULT '';
    ALTER TABLE documents ADD COLUMN s_province TEXT NOT NULL DEFAULT '';
    ALTER TABLE documents ADD COLUMN s_vat TEXT NOT NULL DEFAULT '';
    ALTER TABLE documents ADD COLUMN s_payment TEXT NOT NULL DEFAULT '';
    ALTER TABLE documents ADD COLUMN s_liability TEXT NOT NULL DEFAULT '';
    ALTER TABLE documents ADD COLUMN s_rev INTEGER NOT NULL DEFAULT 0;
    UPDATE documents SET s_legacy_fe = COALESCE(json_extract(data, '$.legacy_fe'), 0), s_order_status = COALESCE(json_extract(data, '$.order_status'), ''), s_expected_date = json_extract(data, '$.expected_date'), s_notes = COALESCE(json_extract(data, '$.internal_notes'), ''), s_city = COALESCE(json_extract(data, '$.party.city'), ''), s_province = COALESCE(json_extract(data, '$.party.province'), ''), s_vat = COALESCE(json_extract(data, '$.party.vat_number'), ''), s_payment = COALESCE(json_extract(data, '$.payment.name'), ''), s_liability = TRIM(COALESCE(json_extract(data, '$.vat_liability'), ''));
    CREATE TRIGGER tr_documents_summary_insert AFTER INSERT ON documents BEGIN
        UPDATE documents SET s_legacy_fe = COALESCE(json_extract(NEW.data, '$.legacy_fe'), 0), s_order_status = COALESCE(json_extract(NEW.data, '$.order_status'), ''), s_expected_date = json_extract(NEW.data, '$.expected_date'), s_notes = COALESCE(json_extract(NEW.data, '$.internal_notes'), ''), s_city = COALESCE(json_extract(NEW.data, '$.party.city'), ''), s_province = COALESCE(json_extract(NEW.data, '$.party.province'), ''), s_vat = COALESCE(json_extract(NEW.data, '$.party.vat_number'), ''), s_payment = COALESCE(json_extract(NEW.data, '$.payment.name'), ''), s_liability = TRIM(COALESCE(json_extract(NEW.data, '$.vat_liability'), '')) WHERE id = NEW.id;
    END;
    CREATE TRIGGER tr_documents_summary_update AFTER UPDATE OF data ON documents BEGIN
        UPDATE documents SET s_rev = s_rev + 1, s_legacy_fe = COALESCE(json_extract(NEW.data, '$.legacy_fe'), 0), s_order_status = COALESCE(json_extract(NEW.data, '$.order_status'), ''), s_expected_date = json_extract(NEW.data, '$.expected_date'), s_notes = COALESCE(json_extract(NEW.data, '$.internal_notes'), ''), s_city = COALESCE(json_extract(NEW.data, '$.party.city'), ''), s_province = COALESCE(json_extract(NEW.data, '$.party.province'), ''), s_vat = COALESCE(json_extract(NEW.data, '$.party.vat_number'), ''), s_payment = COALESCE(json_extract(NEW.data, '$.payment.name'), ''), s_liability = TRIM(COALESCE(json_extract(NEW.data, '$.vat_liability'), '')) WHERE id = NEW.id;
    END;
    CREATE INDEX ix_stock_date ON stock_moves(date, id);
    CREATE INDEX ix_documents_deferred ON documents(date) WHERE s_liability = 'D';
    -- sostituito dall'indice degli elenchi qui sotto, che inizia con le stesse colonne
    DROP INDEX IF EXISTS ix_documents_kind;
    -- indice "che copre" gli elenchi: contiene tutte le colonne che mostrano, così non si
    -- legge la riga del documento (lunga, con il documento completo salvato in `data`)
    CREATE INDEX ix_documents_list ON documents(kind, year, date, number, status, series, party_id, party_name, external_number, total, net_to_pay, agent_id,
        s_legacy_fe, s_order_status, s_expected_date, s_notes, s_city, s_province, s_vat, s_payment, s_liability, s_rev, version, created_at);
    CREATE INDEX ix_stock_product_date ON stock_moves(product_id, date, id);
    CREATE INDEX ix_stock_expiry ON stock_moves(expiry) WHERE lot <> '';
    "#,
    r#"
    -- incassi e pagamenti: una registrazione (data, modalità, conto, riferimento) può
    -- coprire più scadenze, anche solo in parte (acconti)
    CREATE TABLE payments(
        id INTEGER PRIMARY KEY,
        date TEXT NOT NULL,
        party_id INTEGER,
        party_name TEXT NOT NULL DEFAULT '',
        receivable INTEGER NOT NULL,
        mp_code TEXT NOT NULL DEFAULT '',
        account TEXT NOT NULL,
        reference TEXT NOT NULL DEFAULT '',
        notes TEXT NOT NULL DEFAULT '',
        amount TEXT NOT NULL
    );
    CREATE INDEX ix_payments_date ON payments(date);
    CREATE TABLE payment_allocations(
        id INTEGER PRIMARY KEY,
        payment_id INTEGER NOT NULL REFERENCES payments(id) ON DELETE CASCADE,
        due_id INTEGER NOT NULL REFERENCES due_dates(id),
        amount TEXT NOT NULL,
        writeoff TEXT NOT NULL DEFAULT '0'
    );
    CREATE INDEX ix_alloc_payment ON payment_allocations(payment_id);
    CREATE INDEX ix_alloc_due ON payment_allocations(due_id);
    ALTER TABLE cash_entries ADD COLUMN payment_id INTEGER REFERENCES payments(id) ON DELETE SET NULL;
    CREATE INDEX ix_cash_payment ON cash_entries(payment_id);
    CREATE INDEX ix_cash_due ON cash_entries(due_date_id);
    -- i movimenti già registrati sulle scadenze diventano registrazioni (modalità non indicata)
    INSERT INTO payments(id, date, party_id, party_name, receivable, mp_code, account, amount)
        SELECT c.id, c.date, c.party_id, COALESCE(d.party_name, ''), CASE WHEN c.amount LIKE '-%' THEN 0 ELSE 1 END, '', c.account, LTRIM(c.amount, '-')
        FROM cash_entries c JOIN due_dates s ON s.id = c.due_date_id JOIN documents d ON d.id = s.document_id;
    INSERT INTO payment_allocations(payment_id, due_id, amount)
        SELECT c.id, c.due_date_id, LTRIM(c.amount, '-') FROM cash_entries c WHERE c.id IN (SELECT id FROM payments);
    UPDATE cash_entries SET payment_id = id WHERE id IN (SELECT id FROM payments);
    "#,
    // giroconti e righe dell'estratto conto già registrate; registro dei versamenti F24
    r#"
    ALTER TABLE cash_entries ADD COLUMN link TEXT;
    CREATE INDEX ix_cash_link ON cash_entries(link);
    CREATE TABLE f24_payments(
        id INTEGER PRIMARY KEY,
        date TEXT NOT NULL,
        account TEXT NOT NULL,
        total TEXT NOT NULL,
        items TEXT NOT NULL,
        cash_entry_id INTEGER REFERENCES cash_entries(id) ON DELETE SET NULL
    );
    "#,
];

pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let v = (i + 1) as i64;
        if v <= version {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", v)?;
        tx.commit()?;
    }
    Ok(())
}

#[allow(dead_code)]
pub fn schema_version() -> usize {
    MIGRATIONS.len()
}
