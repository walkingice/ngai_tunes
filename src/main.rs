mod client;
mod daemon;
mod playback;
mod protocol;
mod storage;

use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = env!("CARGO_BIN_NAME"), version, about = "An MP3 and FLAC player with terminal and menu clients")]
struct Cli {
    #[arg(long, conflicts_with_all = ["daemon", "directory"])]
    attach: bool,
    #[arg(long, conflicts_with = "directory")]
    daemon: bool,
    #[arg(long, hide = true, conflicts_with_all = ["attach", "daemon", "directory"])]
    detached_child: bool,
    /// Open the menu bar playback controls
    #[arg(long, conflicts_with_all = ["attach", "daemon", "detached_child", "directory"])]
    menu: bool,
    #[arg(value_name = "DIRECTORY")]
    directory: Option<PathBuf>,
}

impl Cli {
    fn opens_menu(&self, bundle_launch: bool) -> bool {
        self.menu
            || (bundle_launch
                && !self.attach
                && !self.daemon
                && !self.detached_child
                && self.directory.is_none())
    }
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}: {error}", env!("CARGO_BIN_NAME"));
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    if cli.daemon || cli.detached_child {
        daemon::run()
    } else if cli.opens_menu(
        cfg!(target_os = "macos")
            && std::env::var_os("NGAI_TUNES_BUNDLE_MENU").is_some_and(|v| v == "1"),
    ) {
        client::menu::run()
    } else {
        client::tui::run(cli.directory)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_the_executable_name_in_cli_help() {
        use clap::CommandFactory;

        assert_eq!(Cli::command().get_name(), "ngai-tunes");
    }

    #[test]
    fn bundle_launch_defaults_to_menu_only_without_arguments() {
        let cli = Cli::try_parse_from(["player"]).unwrap();
        assert!(cli.opens_menu(true));
        assert!(!cli.opens_menu(false));

        for argument in ["--attach", "--daemon", "--detached-child", "."] {
            let cli = Cli::try_parse_from(["player", argument]).unwrap();
            assert!(!cli.opens_menu(true));
        }

        let cli = Cli::try_parse_from(["player", "--menu"]).unwrap();
        assert!(cli.opens_menu(false));
    }

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
        assert!(Cli::try_parse_from(["player", "--menu"]).unwrap().menu);
        for args in [
            vec!["player", "--menu", "--attach"],
            vec!["player", "--menu", "--daemon"],
            vec!["player", "--menu", "--detached-child"],
            vec!["player", "--menu", "."],
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
