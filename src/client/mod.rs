mod scan;

use std::path::PathBuf;

pub fn run(directory: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let directory = directory.ok_or("No daemon is reachable")?;
    for track in scan::scan(&directory)? {
        println!("{}", track.title);
    }
    Ok(())
}
