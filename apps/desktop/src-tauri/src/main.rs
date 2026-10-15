// Sin consola extra en Windows (versión de producción)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    rvmail_desktop_lib::run()
}
