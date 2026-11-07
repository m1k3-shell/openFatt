#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! openFatt - applicazione desktop.

mod app;
mod collaudo;
mod bankfiles;
mod config;
mod demo;
mod printing;
mod reports;
mod mailer;
mod services;
mod ui;
mod vatfiles;

fn main() -> eframe::Result<()> {
    // openfatt --crea-demo <file.gadb>: crea un archivio dimostrativo ed esce
    let args: Vec<String> = std::env::args().collect();
    // openfatt --inventario-esporta <archivio> <n. sessione> <file.xlsx|csv>
    // openfatt --inventario-importa <archivio> <n. sessione> <file>
    if args.len() == 5 && (args[1] == "--inventario-esporta" || args[1] == "--inventario-importa") {
        let code = match ui::inventory::cli(&args[1], &args[2], &args[3], &args[4]) {
            Ok(msg) => {
                println!("{}", msg);
                0
            }
            Err(e) => {
                eprintln!("{}", e);
                1
            }
        };
        std::process::exit(code);
    }
    if args.len() == 3 && args[1] == "--crea-demo" {
        let r = openfatt_store::Store::open(&args[2]).and_then(|s| demo::populate(&s));
        match r {
            Ok(_) => std::process::exit(0),
            Err(e) => {
                eprintln!("{}", e);
                std::process::exit(1)
            }
        }
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("openFatt")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([1000.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native("openFatt", options, Box::new(|cc| Ok(Box::new(app::OpenFattApp::new(cc)))))
}
