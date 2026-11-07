//! Lettura in sola lettura di un archivio Firebird tramite l'interfaccia pubblica del motore
//! (funzioni `isc_*` documentate nella guida API di InterBase/Firebird).
//!
//! Serve a importare i **propri dati** da un gestionale che usa Firebird (interoperabilità):
//! si legge una copia del file, in una transazione di sola lettura che viene annullata alla
//! fine; nessun dato viene scritto. La libreria del motore (`fbembed.dll` o `fbclient.dll`)
//! si carica a runtime dalla cartella indicata: non è distribuita con openFatt.
//!
//! Ogni colonna viene letta convertita in testo dal motore stesso (`CAST ... AS VARCHAR`):
//! numeri con il punto decimale, date `AAAA-MM-GG`, sì/no come 0/1.

use std::collections::BTreeMap;
use std::ffi::c_void;

use serde::{Deserialize, Serialize};

type Status = [isize; 20];
type Handle = u32;

#[repr(C)]
struct XsqlVar {
    sqltype: i16,
    sqlscale: i16,
    sqlsubtype: i16,
    sqllen: i16,
    sqldata: *mut u8,
    sqlind: *mut i16,
    sqlname_length: i16,
    sqlname: [u8; 32],
    relname_length: i16,
    relname: [u8; 32],
    ownname_length: i16,
    ownname: [u8; 32],
    aliasname_length: i16,
    aliasname: [u8; 32],
}

#[repr(C)]
struct XsqldaHeader {
    version: i16,
    sqldaid: [u8; 8],
    sqldabc: i32,
    sqln: i16,
    sqld: i16,
}

#[repr(C)]
struct Teb {
    db: *mut Handle,
    tpb_len: i32,
    tpb: *const u8,
}

const SQL_VARYING: i16 = 448;
const SQL_TEXT: i16 = 452;

type FnAttach = unsafe extern "system" fn(*mut isize, i16, *const u8, *mut Handle, i16, *const u8) -> isize;
type FnDetach = unsafe extern "system" fn(*mut isize, *mut Handle) -> isize;
type FnStartMultiple = unsafe extern "system" fn(*mut isize, *mut Handle, i16, *mut Teb) -> isize;
type FnRollback = unsafe extern "system" fn(*mut isize, *mut Handle) -> isize;
type FnAllocate = unsafe extern "system" fn(*mut isize, *mut Handle, *mut Handle) -> isize;
type FnPrepare = unsafe extern "system" fn(*mut isize, *mut Handle, *mut Handle, u16, *const u8, u16, *mut c_void) -> isize;
type FnDescribe = unsafe extern "system" fn(*mut isize, *mut Handle, u16, *mut c_void) -> isize;
type FnExecute = unsafe extern "system" fn(*mut isize, *mut Handle, *mut Handle, u16, *mut c_void) -> isize;
type FnFetch = unsafe extern "system" fn(*mut isize, *mut Handle, u16, *mut c_void) -> isize;
type FnFree = unsafe extern "system" fn(*mut isize, *mut Handle, u16) -> isize;
type FnInterpret = unsafe extern "system" fn(*mut u8, u32, *mut *const isize) -> i32;

#[cfg(windows)]
mod os {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        pub fn LoadLibraryW(name: *const u16) -> *mut c_void;
        pub fn GetProcAddress(m: *mut c_void, name: *const u8) -> *mut c_void;
        pub fn SetDllDirectoryW(path: *const u16) -> i32;
    }
    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

struct Api {
    attach: FnAttach,
    detach: FnDetach,
    start: FnStartMultiple,
    rollback: FnRollback,
    allocate: FnAllocate,
    prepare: FnPrepare,
    describe: FnDescribe,
    execute: FnExecute,
    fetch: FnFetch,
    free: FnFree,
    interpret: FnInterpret,
}

#[cfg(windows)]
fn load_api(engine_dir: &str) -> Result<Api, String> {
    unsafe {
        let dir = os::wide(engine_dir);
        os::SetDllDirectoryW(dir.as_ptr());
        let mut lib = std::ptr::null_mut();
        for name in ["fbembed.dll", "fbclient.dll"] {
            let full = std::path::Path::new(engine_dir).join(name);
            if full.exists() {
                lib = os::LoadLibraryW(os::wide(&full.to_string_lossy()).as_ptr());
                if !lib.is_null() {
                    break;
                }
            }
        }
        if lib.is_null() {
            return Err(format!(
                "Impossibile caricare il motore Firebird da {} (servono fbembed.dll o fbclient.dll della stessa architettura del lettore: {} bit)",
                engine_dir,
                std::mem::size_of::<usize>() * 8
            ));
        }
        macro_rules! sym {
            ($n:literal) => {{
                let p = os::GetProcAddress(lib, concat!($n, "\0").as_ptr());
                if p.is_null() {
                    return Err(format!("Funzione {} non trovata nel motore Firebird", $n));
                }
                std::mem::transmute::<*mut c_void, _>(p)
            }};
        }
        Ok(Api {
            attach: sym!("isc_attach_database"),
            detach: sym!("isc_detach_database"),
            start: sym!("isc_start_multiple"),
            rollback: sym!("isc_rollback_transaction"),
            allocate: sym!("isc_dsql_allocate_statement"),
            prepare: sym!("isc_dsql_prepare"),
            describe: sym!("isc_dsql_describe"),
            execute: sym!("isc_dsql_execute"),
            fetch: sym!("isc_dsql_fetch"),
            free: sym!("isc_dsql_free_statement"),
            interpret: sym!("fb_interpret"),
        })
    }
}

#[cfg(not(windows))]
fn load_api(_engine_dir: &str) -> Result<Api, String> {
    Err("Lettura degli archivi Firebird disponibile solo su Windows".into())
}

/// Collegamento in sola lettura a un archivio.
pub struct Database {
    api: Api,
    status: Status,
    db: Handle,
    tr: Handle,
}

/// Contenuto di una tabella: nomi delle colonne e righe (valori testuali o nulli).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TableDump {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

impl TableDump {
    pub fn col(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.eq_ignore_ascii_case(name))
    }
    /// Valore di una colonna nella riga (stringa vuota se nullo o assente).
    pub fn get<'a>(&self, row: &'a [Option<String>], name: &str) -> &'a str {
        self.col(name).and_then(|i| row.get(i)).and_then(|v| v.as_deref()).unwrap_or("")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Dump {
    pub source: String,
    pub tables: BTreeMap<String, TableDump>,
}

/// Decodifica testo: UTF-8 se valido, altrimenti Windows-1252 (codifica usuale degli archivi).
pub fn decode(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b
            .iter()
            .map(|&c| match c {
                0x80 => '€',
                0x8A => 'Š',
                0x8C => 'Œ',
                0x8E => 'Ž',
                0x91 | 0x92 => '\'',
                0x93 | 0x94 => '"',
                0x96 => '–',
                0x97 => '—',
                0x9A => 'š',
                0x9C => 'œ',
                0x9E => 'ž',
                0x9F => 'Ÿ',
                c => c as char,
            })
            .collect(),
    }
}

fn quote(id: &str) -> String {
    format!("\"{}\"", id.replace('"', "\"\""))
}

impl Database {
    /// Apre il file (che dovrebbe essere una copia) con il motore nella cartella indicata.
    pub fn open(engine_dir: &str, file: &str) -> Result<Database, String> {
        let api = load_api(engine_dir)?;
        let mut d = Database { api, status: [0; 20], db: 0, tr: 0 };
        // DPB: versione 1, utente SYSDBA (l'accesso al motore incorporato è dato dal file)
        let dpb: Vec<u8> = vec![1, 28, 6, b'S', b'Y', b'S', b'D', b'B', b'A'];
        let name = file.as_bytes();
        let rc = unsafe { (d.api.attach)(d.status.as_mut_ptr(), name.len() as i16, name.as_ptr(), &mut d.db, dpb.len() as i16, dpb.as_ptr()) };
        d.check(rc, "apertura dell'archivio")?;
        // TPB: versione 3, sola lettura, read committed, rec_version, wait
        let tpb: [u8; 5] = [3, 8, 15, 17, 6];
        let mut teb = Teb { db: &mut d.db, tpb_len: tpb.len() as i32, tpb: tpb.as_ptr() };
        let rc = unsafe { (d.api.start)(d.status.as_mut_ptr(), &mut d.tr, 1, &mut teb) };
        d.check(rc, "avvio della transazione di sola lettura")?;
        Ok(d)
    }

    fn check(&mut self, rc: isize, what: &str) -> Result<(), String> {
        if rc == 0 {
            return Ok(());
        }
        let mut msgs = Vec::new();
        let mut p: *const isize = self.status.as_ptr();
        let mut buf = vec![0u8; 1024];
        unsafe {
            while (self.api.interpret)(buf.as_mut_ptr(), buf.len() as u32, &mut p) > 0 {
                let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                msgs.push(decode(&buf[..end]));
                if msgs.len() > 8 {
                    break;
                }
            }
        }
        Err(format!("Firebird, {}: {}", what, msgs.join("; ")))
    }

    /// Esegue una SELECT le cui colonne sono tutte testuali; restituisce le righe.
    pub fn query_text(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        if !sql.trim_start().to_uppercase().starts_with("SELECT ") {
            return Err("Sono ammesse solo interrogazioni SELECT".into());
        }
        let mut stmt: Handle = 0;
        let rc = unsafe { (self.api.allocate)(self.status.as_mut_ptr(), &mut self.db, &mut stmt) };
        self.check(rc, "preparazione")?;
        let result = self.run(stmt, sql);
        unsafe {
            (self.api.free)(self.status.as_mut_ptr(), &mut stmt, 2);
        }
        result
    }

    fn run(&mut self, mut stmt: Handle, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        let hdr = std::mem::size_of::<XsqldaHeader>().next_multiple_of(std::mem::align_of::<XsqlVar>());
        let var = std::mem::size_of::<XsqlVar>();
        let alloc = |n: usize| -> Vec<u64> { vec![0u64; (hdr + var * n.max(1)).div_ceil(8)] };
        let mut n = 1usize;
        let mut da = alloc(n);
        let set_header = |da: &mut Vec<u64>, n: usize| unsafe {
            let h = da.as_mut_ptr() as *mut XsqldaHeader;
            (*h).version = 1;
            (*h).sqln = n as i16;
            (*h).sqldabc = (hdr + var * n) as i32;
        };
        set_header(&mut da, n);
        let q = sql.as_bytes();
        let rc = unsafe { (self.api.prepare)(self.status.as_mut_ptr(), &mut self.tr, &mut stmt, q.len() as u16, q.as_ptr(), 3, da.as_mut_ptr() as *mut c_void) };
        self.check(rc, "interrogazione")?;
        let sqld = unsafe { (*(da.as_ptr() as *const XsqldaHeader)).sqld } as usize;
        if sqld > n {
            n = sqld;
            da = alloc(n);
            set_header(&mut da, n);
            let rc = unsafe { (self.api.describe)(self.status.as_mut_ptr(), &mut stmt, 1, da.as_mut_ptr() as *mut c_void) };
            self.check(rc, "descrizione delle colonne")?;
        }
        // buffer dei valori
        let vars = unsafe { std::slice::from_raw_parts_mut((da.as_mut_ptr() as *mut u8).add(hdr) as *mut XsqlVar, sqld) };
        let mut data: Vec<Vec<u8>> = Vec::with_capacity(sqld);
        let mut inds: Vec<i16> = vec![0; sqld];
        for (i, v) in vars.iter_mut().enumerate() {
            let t = v.sqltype & !1;
            if t != SQL_VARYING && t != SQL_TEXT {
                return Err(format!("Colonna {} non testuale (tipo {}): usare CAST", i + 1, t));
            }
            data.push(vec![0u8; v.sqllen as usize + 2]);
        }
        for (i, v) in vars.iter_mut().enumerate() {
            v.sqldata = data[i].as_mut_ptr();
            v.sqlind = &mut inds[i];
        }
        let rc = unsafe { (self.api.execute)(self.status.as_mut_ptr(), &mut self.tr, &mut stmt, 3, std::ptr::null_mut()) };
        self.check(rc, "esecuzione")?;
        let mut rows = Vec::new();
        loop {
            inds.iter_mut().for_each(|x| *x = 0);
            let rc = unsafe { (self.api.fetch)(self.status.as_mut_ptr(), &mut stmt, 1, da.as_mut_ptr() as *mut c_void) };
            if rc == 100 {
                break;
            }
            self.check(rc, "lettura delle righe")?;
            let vars = unsafe { std::slice::from_raw_parts((da.as_ptr() as *const u8).add(hdr) as *const XsqlVar, sqld) };
            let mut row = Vec::with_capacity(sqld);
            for (i, v) in vars.iter().enumerate() {
                if (v.sqltype & 1) == 1 && inds[i] < 0 {
                    row.push(None);
                    continue;
                }
                let b = &data[i];
                let s = if (v.sqltype & !1) == SQL_VARYING {
                    let len = u16::from_le_bytes([b[0], b[1]]) as usize;
                    decode(&b[2..2 + len.min(b.len() - 2)])
                } else {
                    decode(&b[..v.sqllen as usize]).trim_end().to_string()
                };
                row.push(Some(s));
            }
            rows.push(row);
        }
        Ok(rows)
    }

    /// Colonne di una tabella con il tipo e la sottotipo (dal catalogo di sistema).
    pub fn columns(&mut self, table: &str) -> Result<Vec<(String, i32, i32, i32)>, String> {
        let sql = format!(
            "SELECT CAST(TRIM(rf.RDB$FIELD_NAME) AS VARCHAR(100)), CAST(f.RDB$FIELD_TYPE AS VARCHAR(10)), CAST(COALESCE(f.RDB$FIELD_SUB_TYPE, 0) AS VARCHAR(10)), CAST(COALESCE(f.RDB$CHARACTER_LENGTH, f.RDB$FIELD_LENGTH, 0) AS VARCHAR(10)) \
             FROM RDB$RELATION_FIELDS rf JOIN RDB$FIELDS f ON f.RDB$FIELD_NAME = rf.RDB$FIELD_SOURCE WHERE rf.RDB$RELATION_NAME = '{}' ORDER BY rf.RDB$FIELD_POSITION",
            table.replace('\'', "''")
        );
        Ok(self
            .query_text(&sql)?
            .into_iter()
            .map(|r| {
                let g = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
                (g(0), g(1).trim().parse().unwrap_or(0), g(2).trim().parse().unwrap_or(0), g(3).trim().parse().unwrap_or(0))
            })
            .collect())
    }

    /// Tutte le righe di una tabella, ogni colonna convertita in testo (le immagini sono escluse).
    pub fn dump_table(&mut self, table: &str) -> Result<TableDump, String> {
        let cols = self.columns(table)?;
        if cols.is_empty() {
            return Err(format!("Tabella {} non trovata", table));
        }
        let mut names = Vec::new();
        let mut exprs = Vec::new();
        for (name, ty, sub, len) in cols {
            let expr = match ty {
                261 if sub == 1 => format!("CAST({} AS VARCHAR(32000))", quote(&name)),
                261 => continue,
                37 | 14 => format!("CAST({} AS VARCHAR({}))", quote(&name), len.clamp(1, 32000)),
                _ => format!("CAST({} AS VARCHAR(60))", quote(&name)),
            };
            names.push(name);
            exprs.push(expr);
        }
        let sql = format!("SELECT {} FROM {}", exprs.join(", "), quote(table));
        let rows = self.query_text(&sql)?;
        Ok(TableDump { columns: names, rows })
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        unsafe {
            if self.tr != 0 {
                (self.api.rollback)(self.status.as_mut_ptr(), &mut self.tr);
            }
            if self.db != 0 {
                (self.api.detach)(self.status.as_mut_ptr(), &mut self.db);
            }
        }
    }
}

/// Tabelle lette per l'importazione da Easyfatt.
pub const EASYFATT_TABLES: [&str; 14] = [
    "TConfig",
    "TIva",
    "TPagamenti",
    "TAgenti",
    "TAnagrafica",
    "TAnagraficaDest",
    "TArticoli",
    "TArticoliCodBarre",
    "TDiba",
    "TDocTestate",
    "TDocRighe",
    "TPrimaNota",
    "TMovMagazz",
    "TCategorie",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_sizes_match_the_c_api() {
        // XSQLVAR e testata di XSQLDA come in ibase.h (32 bit: 152 e 20 byte)
        if std::mem::size_of::<usize>() == 4 {
            assert_eq!(std::mem::size_of::<XsqlVar>(), 152);
            assert_eq!(std::mem::size_of::<XsqldaHeader>(), 20);
        }
        assert_eq!(decode(b"Citt\xe0"), "Città");
        assert_eq!(decode("Città".as_bytes()), "Città");
        let t = TableDump { columns: vec!["A".into(), "B".into()], rows: vec![vec![Some("1".into()), None]] };
        assert_eq!(t.get(&t.rows[0], "a"), "1");
        assert_eq!(t.get(&t.rows[0], "B"), "");
    }
}
