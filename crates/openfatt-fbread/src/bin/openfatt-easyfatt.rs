//! Lettore ausiliario degli archivi di Easyfatt (versione a 32 bit, come il motore Firebird
//! installato con Easyfatt). Copia l'archivio in una cartella temporanea, lo legge in sola
//! lettura e scrive il contenuto delle tabelle utili all'importazione in un file JSON.
//!
//! Uso: openfatt-easyfatt <archivio.eft> <cartella del motore Firebird> <uscita.json>

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("Uso: openfatt-easyfatt <archivio> <cartella del motore Firebird> <uscita.json>");
        std::process::exit(2);
    }
    match run(&args[1], &args[2], &args[3]) {
        Ok(n) => println!("OK {} tabelle", n),
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}

fn run(archive: &str, engine: &str, out: &str) -> Result<usize, String> {
    // si lavora su una copia: il motore aggiorna l'intestazione anche in sola lettura
    let tmp = std::env::temp_dir().join(format!("openfatt-lettura-{}.fdb", std::process::id()));
    std::fs::copy(archive, &tmp).map_err(|e| format!("Copia dell'archivio: {}", e))?;
    let result = (|| {
        let mut db = openfatt_fbread::Database::open(engine, &tmp.to_string_lossy())?;
        let mut dump = openfatt_fbread::Dump { source: PathBuf::from(archive).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(), ..Default::default() };
        for t in openfatt_fbread::EASYFATT_TABLES {
            match db.dump_table(t) {
                Ok(d) => {
                    dump.tables.insert(t.to_string(), d);
                }
                Err(e) if e.contains("non trovata") => {}
                Err(e) => return Err(format!("{}: {}", t, e)),
            }
        }
        drop(db);
        let n = dump.tables.len();
        std::fs::write(out, serde_json::to_vec(&dump).map_err(|e| e.to_string())?).map_err(|e| format!("Scrittura di {}: {}", out, e))?;
        Ok(n)
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}
