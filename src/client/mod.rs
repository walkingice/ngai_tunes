mod ipc;
mod library;
mod recursive_scan;
mod scan;
pub(crate) mod tui;

pub(crate) mod menu;

fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
