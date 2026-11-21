//! Skin Studio: one window, one big live gameplay view, lazer-style settings on the side.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
#[cfg(test)]
mod audit;
mod element_creator;
mod motion;
mod skin_editor;
mod skin_ini;
mod paths;
mod state;
mod studio;
mod theme;
mod ui;

fn main() {
    app::run();
}
