#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = copycat_desktop::firewall::cli() {
        std::process::exit(code);
    }
    copycat_desktop::run();
}
