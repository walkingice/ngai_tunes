mod ipc;
mod scan;
mod ui;

use ipc::Connection;
use std::path::PathBuf;

pub fn run(directory: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let (tracks, connection) = if let Some(path) = &directory {
        (scan::scan(path)?, Connection::connect().ok())
    } else {
        (
            Vec::new(),
            Some(
                Connection::connect()
                    .map_err(|error| format!("No daemon is reachable: {error}"))?,
            ),
        )
    };
    let mut app = ui::App::new(directory, tracks, connection);
    app.run()?;
    Ok(())
}
