mod ipc;
mod recursive_scan;
mod scan;
mod ui;

use ipc::Connection;
use std::path::PathBuf;

pub fn run(directory: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let tracks = if let Some(path) = &directory {
        scan::scan(path)?
    } else {
        Vec::new()
    };
    let mut app = ui::App::new(directory, tracks, Connection::connect().ok());
    app.run()?;
    Ok(())
}
