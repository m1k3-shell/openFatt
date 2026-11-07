//! File di inventario da compilare in Excel (o in un foglio di calcolo qualsiasi).
//!
//! L'esportazione contiene TUTTE le righe della sessione, comprese quelle con giacenza zero
//! o negativa: è proprio su quelle che spesso si trovano differenze. La colonna "Contato" è da
//! compilare; una cella vuota significa "non contato". La colonna "ID" (nascosta e protetta
//! dalla posizione) permette di ritrovare la riga anche se il file viene ordinato o filtrato;
//! in sua assenza si usa la combinazione codice + variante + lotto + seriale.

use std::path::Path;
use std::str::FromStr;

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

/// Riga di inventario da esportare.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InventoryRow {
    pub line_id: i64,
    pub code: String,
    pub description: String,
    pub category: String,
    pub variant: String,
    pub lot: String,
    pub expiry: String,
    pub serial: String,
    pub unit: String,
    pub barcode: String,
    pub cost: Decimal,
    pub expected: Decimal,
    pub counted: Option<Decimal>,
    pub note: String,
}

/// Conteggio letto da un file compilato.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CountedRow {
    /// Numero di riga nel file (per i messaggi)
    pub file_row: usize,
    pub line_id: Option<i64>,
    pub code: String,
    pub variant: String,
    pub lot: String,
    pub expiry: String,
    pub serial: String,
    /// None = cella vuota (non contato)
    pub counted: Option<Decimal>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportResult {
    pub rows: Vec<CountedRow>,
    pub warnings: Vec<String>,
}

pub const HEADERS: [&str; 14] = [
    "ID", "Codice", "Descrizione", "Categoria", "Variante", "Lotto", "Scadenza", "Seriale", "U.M.", "Codice a barre", "Costo", "Giacenza teorica", "Contato", "Note",
];

fn to_f64(d: Decimal) -> f64 {
    d.to_f64().unwrap_or(0.0)
}

/// Scrive il file Excel (.xlsx) con il foglio da compilare e un foglio di istruzioni.
pub fn export_xlsx(path: &Path, title: &str, rows: &[InventoryRow]) -> Result<(), String> {
    use rust_xlsxwriter::{Color, Format, FormatBorder, Workbook};
    let mut wb = Workbook::new();
    let head = Format::new().set_bold().set_background_color(Color::RGB(0xDDE5F0)).set_border(FormatBorder::Thin);
    let count_head = Format::new().set_bold().set_background_color(Color::RGB(0xFFE699)).set_border(FormatBorder::Thin);
    let count_cell = Format::new().set_background_color(Color::RGB(0xFFF2CC)).set_num_format("General").set_border(FormatBorder::Thin);
    let qty = Format::new().set_num_format("General");
    let money = Format::new().set_num_format("#,##0.00");
    let id_fmt = Format::new().set_font_color(Color::RGB(0x999999));
    let e = |r: Result<&mut rust_xlsxwriter::Worksheet, rust_xlsxwriter::XlsxError>| r.map(|_| ()).map_err(|x| x.to_string());

    let ws = wb.add_worksheet();
    ws.set_name("Inventario").map_err(|x| x.to_string())?;
    for (c, h) in HEADERS.iter().enumerate() {
        let f = if *h == "Contato" { &count_head } else { &head };
        e(ws.write_string_with_format(0, c as u16, *h, f))?;
    }
    for (i, r) in rows.iter().enumerate() {
        let row = (i + 1) as u32;
        e(ws.write_number_with_format(row, 0, r.line_id as f64, &id_fmt))?;
        e(ws.write_string(row, 1, &r.code))?;
        e(ws.write_string(row, 2, &r.description))?;
        e(ws.write_string(row, 3, &r.category))?;
        e(ws.write_string(row, 4, &r.variant))?;
        e(ws.write_string(row, 5, &r.lot))?;
        e(ws.write_string(row, 6, &r.expiry))?;
        e(ws.write_string(row, 7, &r.serial))?;
        e(ws.write_string(row, 8, &r.unit))?;
        e(ws.write_string(row, 9, &r.barcode))?;
        e(ws.write_number_with_format(row, 10, to_f64(r.cost), &money))?;
        e(ws.write_number_with_format(row, 11, to_f64(r.expected), &qty))?;
        match r.counted {
            Some(c) => e(ws.write_number_with_format(row, 12, to_f64(c), &count_cell))?,
            None => e(ws.write_blank(row, 12, &count_cell))?,
        }
        e(ws.write_string(row, 13, &r.note))?;
    }
    let widths = [7.0, 14.0, 42.0, 16.0, 14.0, 12.0, 11.0, 16.0, 6.0, 15.0, 10.0, 15.0, 12.0, 30.0];
    for (c, w) in widths.iter().enumerate() {
        ws.set_column_width(c as u16, *w).map_err(|x| x.to_string())?;
    }
    ws.set_freeze_panes(1, 3).map_err(|x| x.to_string())?;
    if !rows.is_empty() {
        ws.autofilter(0, 0, rows.len() as u32, (HEADERS.len() - 1) as u16).map_err(|x| x.to_string())?;
    }

    let info = wb.add_worksheet();
    info.set_name("Istruzioni").map_err(|x| x.to_string())?;
    let lines = [
        title.to_string(),
        String::new(),
        "1. Scrivere nella colonna gialla 'Contato' la quantità effettivamente presente.".to_string(),
        "2. Lasciare vuota la cella degli articoli non contati (zero = articolo contato e assente).".to_string(),
        "3. Il file contiene tutti gli articoli gestiti a magazzino, anche quelli con giacenza zero o negativa.".to_string(),
        "4. Si possono ordinare e filtrare le righe; non modificare la colonna ID.".to_string(),
        "5. Per un lotto o un numero di serie trovato ma non presente: aggiungere una riga con codice, lotto/seriale e quantità, lasciando vuoto l'ID.".to_string(),
        "6. Salvare il file e importarlo da openFatt > Inventario > Importa conteggi.".to_string(),
    ];
    for (i, l) in lines.iter().enumerate() {
        info.write_string(i as u32, 0, l).map_err(|x| x.to_string())?;
    }
    info.set_column_width(0, 110.0).map_err(|x| x.to_string())?;
    wb.save(path).map_err(|x| format!("Salvataggio del file Excel: {}", x))
}

fn csv_field(s: &str) -> String {
    if s.contains([';', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn it_num(d: Decimal) -> String {
    d.normalize().to_string().replace('.', ",")
}

/// File CSV con separatore ";" e decimali con la virgola (apribile da Excel in italiano).
pub fn export_csv(rows: &[InventoryRow]) -> String {
    let mut o = String::from("\u{feff}");
    o.push_str(&HEADERS.join(";"));
    o.push('\n');
    for r in rows {
        let fields = [
            r.line_id.to_string(),
            csv_field(&r.code),
            csv_field(&r.description),
            csv_field(&r.category),
            csv_field(&r.variant),
            csv_field(&r.lot),
            csv_field(&r.expiry),
            csv_field(&r.serial),
            csv_field(&r.unit),
            csv_field(&r.barcode),
            it_num(r.cost),
            it_num(r.expected),
            r.counted.map(it_num).unwrap_or_default(),
            csv_field(&r.note),
        ];
        o.push_str(&fields.join(";"));
        o.push('\n');
    }
    o
}

fn norm(h: &str) -> String {
    h.trim().to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Indici delle colonne riconosciute dall'intestazione (l'ordine nel file è libero).
struct Cols {
    id: Option<usize>,
    code: Option<usize>,
    variant: Option<usize>,
    lot: Option<usize>,
    expiry: Option<usize>,
    serial: Option<usize>,
    counted: Option<usize>,
    note: Option<usize>,
}

fn find_cols(head: &[String]) -> Cols {
    let f = |names: &[&str]| head.iter().position(|h| names.iter().any(|n| norm(h) == norm(n)));
    Cols {
        id: f(&["ID", "ID riga", "riga"]),
        code: f(&["Codice", "Codice articolo", "Cod"]),
        variant: f(&["Variante", "Taglia/colore"]),
        lot: f(&["Lotto"]),
        expiry: f(&["Scadenza", "Scadenza lotto"]),
        serial: f(&["Seriale", "Numero di serie", "Matricola"]),
        counted: f(&["Contato", "Quantità contata", "Qta contata", "Conteggio", "Quantità rilevata"]),
        note: f(&["Note", "Nota"]),
    }
}

fn parse_qty(s: &str) -> Result<Option<Decimal>, String> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(None);
    }
    openfatt_core::money::parse_dec(t).map(Some).ok_or_else(|| format!("quantità '{}' non valida", t))
}

fn rows_to_counts(table: Vec<Vec<String>>) -> Result<ImportResult, String> {
    let mut it = table.into_iter().enumerate();
    // l'intestazione è la prima riga che contiene la colonna "Contato"
    let mut head = None;
    for (_, r) in it.by_ref() {
        if find_cols(&r).counted.is_some() {
            head = Some(r);
            break;
        }
    }
    let head = head.ok_or("Nel file manca la colonna 'Contato'")?;
    let c = find_cols(&head);
    if c.id.is_none() && c.code.is_none() {
        return Err("Nel file mancano sia la colonna 'ID' sia la colonna 'Codice'".into());
    }
    let counted_col = c.counted.unwrap();
    let mut res = ImportResult::default();
    for (i, r) in it {
        let get = |col: Option<usize>| col.and_then(|x| r.get(x)).map(|s| s.trim().to_string()).unwrap_or_default();
        if r.iter().all(|x| x.trim().is_empty()) {
            continue;
        }
        let counted = match parse_qty(&get(Some(counted_col))) {
            Ok(v) => v,
            Err(e) => {
                res.warnings.push(format!("Riga {}: {}", i + 1, e));
                continue;
            }
        };
        let id = get(c.id);
        let line_id = if id.is_empty() {
            None
        } else {
            match Decimal::from_str(&id.replace(',', ".")).ok().and_then(|d| d.to_i64()) {
                Some(v) => Some(v),
                None => {
                    res.warnings.push(format!("Riga {}: ID '{}' non valido", i + 1, id));
                    continue;
                }
            }
        };
        res.rows.push(CountedRow {
            file_row: i + 1,
            line_id,
            code: get(c.code),
            variant: get(c.variant),
            lot: get(c.lot),
            expiry: get(c.expiry),
            serial: get(c.serial),
            counted,
            note: get(c.note),
        });
    }
    Ok(res)
}

/// Legge un file compilato: Excel (.xlsx, .xlsm, .xls), OpenDocument (.ods) o CSV.
pub fn import_counts(path: &Path) -> Result<ImportResult, String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let table: Vec<Vec<String>> = if ext == "csv" || ext == "txt" {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        crate::csv::parse_csv(&openfatt_fatturapa::parse::decode_xml_bytes(&bytes))
    } else {
        use calamine::{open_workbook_auto, Data, Reader};
        let mut wb = open_workbook_auto(path).map_err(|e| format!("File non leggibile: {}", e))?;
        let names = wb.sheet_names().to_vec();
        let sheet = names.iter().find(|n| n.eq_ignore_ascii_case("Inventario")).or_else(|| names.first()).ok_or("Il file non contiene fogli")?.clone();
        let range = wb.worksheet_range(&sheet).map_err(|e| format!("Foglio '{}': {}", sheet, e))?;
        range
            .rows()
            .map(|r| {
                r.iter()
                    .map(|c| match c {
                        Data::Empty => String::new(),
                        Data::String(s) => s.clone(),
                        // i numeri si convertono senza perdere i decimali significativi
                        Data::Float(f) => Decimal::from_f64_retain(*f).map(|d| openfatt_core::money::round_dp(d, 6).normalize().to_string()).unwrap_or_default(),
                        Data::Int(i) => i.to_string(),
                        Data::Bool(b) => b.to_string(),
                        other => other.to_string(),
                    })
                    .collect()
            })
            .collect()
    };
    rows_to_counts(table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros_shim::dec;

    mod rust_decimal_macros_shim {
        pub use openfatt_core::dec;
    }

    fn sample() -> Vec<InventoryRow> {
        vec![
            InventoryRow { line_id: 1, code: "A".into(), description: "Con giacenza".into(), expected: dec!(5), cost: dec!(2.5), ..Default::default() },
            InventoryRow { line_id: 2, code: "B".into(), description: "Giacenza zero".into(), expected: dec!(0), ..Default::default() },
            InventoryRow { line_id: 3, code: "C".into(), description: "Negativa; con \"virgolette\"".into(), expected: dec!(-2), counted: Some(dec!(1.5)), ..Default::default() },
        ]
    }

    #[test]
    fn xlsx_round_trip_keeps_zero_rows() {
        let dir = std::env::temp_dir().join(format!("ga-inv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("inventario.xlsx");
        export_xlsx(&p, "Prova", &sample()).unwrap();
        let back = import_counts(&p).unwrap();
        // tutte le righe, anche quelle a zero, tornano indietro
        assert_eq!(back.rows.len(), 3, "{:?}", back.warnings);
        assert_eq!(back.rows[1].code, "B");
        assert_eq!(back.rows[1].line_id, Some(2));
        assert_eq!(back.rows[1].counted, None);
        assert_eq!(back.rows[2].counted, Some(dec!(1.5)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn csv_round_trip_and_free_column_order() {
        let csv = export_csv(&sample());
        assert!(csv.contains(";B;Giacenza zero;"));
        let table = crate::csv::parse_csv(&csv);
        let res = rows_to_counts(table).unwrap();
        assert_eq!(res.rows.len(), 3);
        assert_eq!(res.rows[2].counted, Some(dec!(1.5)));
        // file compilato a mano con colonne in altro ordine, senza ID e con decimali italiani
        let manual = "Note;Contato;Lotto;Codice\nok;3;L1;A\n;2,5;;B\n;abc;;C\n";
        let res = rows_to_counts(crate::csv::parse_csv(manual)).unwrap();
        assert_eq!(res.rows.len(), 2);
        assert_eq!(res.rows[0].lot, "L1");
        assert_eq!(res.rows[1].counted, Some(dec!(2.5)));
        assert_eq!(res.warnings.len(), 1);
        assert!(rows_to_counts(crate::csv::parse_csv("Codice;Quantita\nA;1\n")).is_err());
    }
}
