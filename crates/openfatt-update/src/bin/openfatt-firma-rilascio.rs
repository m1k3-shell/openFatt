//! Strumento per chi pubblica le versioni: crea la chiave e firma l'installatore.
//!
//! Uso: openfatt-firma-rilascio genera <file chiave>
//!      openfatt-firma-rilascio firma <file chiave> <installatore.exe> <versione> <manifesto.json>
//!
//! La chiave privata non deve mai finire nel progetto: chi la possiede può firmare aggiornamenti.

use std::path::Path;

fn run(args: &[String]) -> Result<(), String> {
    match args.iter().map(|s| s.as_str()).collect::<Vec<_>>().as_slice() {
        ["genera", key] => {
            let key = Path::new(key);
            if key.exists() {
                return Err(format!("{} esiste già: non viene sovrascritta", key.display()));
            }
            if let Some(d) = key.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            let k = openfatt_update::generate_key()?;
            std::fs::write(key, &k).map_err(|e| e.to_string())?;
            println!("{}", openfatt_update::public_key_hex(&k)?);
            Ok(())
        }
        ["firma", key, file, version, out] => {
            let k = std::fs::read(key).map_err(|e| format!("{}: {}", key, e))?;
            let m = openfatt_update::sign_file(&k, Path::new(file), version)?;
            std::fs::write(out, serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            println!("firmato {} {}", m.file, m.sha256);
            Ok(())
        }
        _ => Err("Uso: genera <chiave> | firma <chiave> <installatore> <versione> <manifesto>".into()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
