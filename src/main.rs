mod client;
mod daemon;
mod playback;
mod protocol;

use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(version, about = "A terminal MP3 and FLAC player")]
struct Cli {
    #[arg(long, conflicts_with_all = ["daemon", "directory"])]
    attach: bool,
    #[arg(long, conflicts_with = "directory")]
    daemon: bool,
    #[arg(long, hide = true, conflicts_with_all = ["attach", "daemon", "directory"])]
    detached_child: bool,
    #[arg(value_name = "DIRECTORY")]
    directory: Option<PathBuf>,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ngai_tunes: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    if cli.daemon || cli.detached_child {
        daemon::run()
    } else {
        client::run(cli.directory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modes_and_paths() {
        for args in [vec!["player"], vec!["player", "--attach"]] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(cli.directory.is_none() && !cli.daemon);
        }
        for (args, path) in [
            (vec!["player", "."], "."),
            (vec!["player", "attach"], "attach"),
            (vec!["player", "--", "-music"], "-music"),
        ] {
            assert_eq!(
                Cli::try_parse_from(args).unwrap().directory,
                Some(path.into())
            );
        }
        assert!(Cli::try_parse_from(["player", "--daemon"]).unwrap().daemon);
        for args in [
            vec!["player", "--attach", "--daemon"],
            vec!["player", "--attach", "."],
            vec!["player", "--daemon", "."],
            vec!["player", ".", "other"],
            vec!["player", "--unknown"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }
}
