// Release builds are GUI apps on Windows: no console window opens next to the app.
// Debug builds keep the console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() { dissent_desktop_lib::run() }
