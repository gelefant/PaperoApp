#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--ml-worker") {
        if let Err(e) = papero_core::ml::run_ml_worker_stdio() {
            eprintln!("papero ml-worker error: {e}");
            std::process::exit(1);
        }
        return;
    }
    papero_app_lib::run();
}
