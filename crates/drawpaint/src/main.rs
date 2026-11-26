// Release builds on Windows are GUI apps without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod gpu;
mod pointer;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    app::run()
}
