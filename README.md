# ngai_tunes

ngai_tunes is a keyboard-driven MP3 and FLAC player for Linux and
macOS. Its terminal and menu clients share a local daemon so music can
continue after the terminal closes.

## Build

Build from the repository root with Rust and Cargo. On Linux, install
`pkg-config` and the ALSA development headers (`libasound2-dev` on Debian
and Ubuntu, or `alsa-lib-devel` on Fedora). macOS uses Core Audio.

```sh
cargo build --release
```

## Usage

```sh
./target/release/ngai-tunes               # Attach to the running queue
./target/release/ngai-tunes --attach      # Explicitly attach to the running queue
./target/release/ngai-tunes .             # Scan the working directory
./target/release/ngai-tunes ~/Music/Jazz  # Scan one directory
./target/release/ngai-tunes --daemon      # Run the daemon in the foreground
./target/release/ngai-tunes --menu        # Open native menu playback controls
```

Passing a directory opens Browsing. Without a directory, the app opens Scanned
if `database.jsonl` exists in the config directory, and Playing otherwise.
If no daemon is running, the Playing queue is empty. Launching without a
directory also shows a message at the bottom in that case.
To start a session, pass a directory. In the Browsing tab, select a track
with the arrow keys and press Enter to load the list into Playing and start
playback. Playing shows the daemon queue and starts empty without a daemon.
Space pauses or resumes. Esc detaches and asks a manually stopped daemon to exit.
Press `q` to stop a playing or paused track. Press `q` again once playback
has stopped to close the client and terminate the daemon.
With repeat off, the daemon exits when it reaches the end of the queue.

In Playing, Left/Right seek by five seconds while playing or paused.
Vim keys `h`, `j`, `k`, and `l` work as Left, Down, Up, and Right.
Tab switches focus between Main and Playback. Enter in Playback stops or
restarts the retained track.
Press uppercase `P` to focus Playback or uppercase `S` to focus the visible
song list.
`n` moves to the next track, and `N` or `p` moves to the previous track.
Left/Right seek by five seconds, `r` cycles repeat, and `o` toggles playback order. Repeat-one
applies to natural endings. Manual next and previous follow play order.
In random order, previous retraces the current shuffled cycle. Repeat-off
exits at the end of the cycle, and repeat-all starts a new shuffle.

Press uppercase `T` to focus Tabs, then Up/Down to select Settings and Enter
to open it. Press Tab from Tabs to return to Main.
Select a directory path to edit it, or select its Delete button to confirm
removal. Add directory creates a target. The Scan button asks for confirmation; scanning is not implemented yet.
Targets are saved in `config.json` under `scan_directories`.

In Scanned, Tab cycles from the song list through the filters and Playback.
Press uppercase `R` to reset all filters to `All`.

See the [SPEC](AGENTS.md) and [architecture](docs/ARCH.md).

## Menu client

Start the menu client independently of the TUI. While the server is offline,
**Play** starts it and plays the selected track from the saved queue. Use the
TUI to add songs when the queue is empty. **Exit** shuts down the daemon and
closes the menu client. An open TUI remains usable.

### Build the macOS menu bar app

Install [cargo-bundle] once, then run the bundle script from the repository
root. The script builds the release binary and packages it as `Ngai Tunes.app`.

```sh
cargo install --locked cargo-bundle
sh src/client/menu/bundle-macos.sh
```

With the default Cargo target directory, open the app from Finder or run:

```sh
open "target/release/bundle/osx/Ngai Tunes.app"
```

Click the music note in the menu bar to open the playback controls. To run
the menu client directly from a terminal, use `cargo run --release -- --menu`.

### Run the Linux menu client

On Linux, run `ngai-tunes --menu` from a desktop session with a
StatusNotifierItem host. If the process runs but no icon appears, check for
a host on the session bus.

```sh
busctl --user get-property org.kde.StatusNotifierWatcher /StatusNotifierWatcher \
    org.kde.StatusNotifierWatcher IsStatusNotifierHostRegistered
```

A missing service or `b false` means the desktop needs a panel or extension
that provides a StatusNotifierItem host.

[cargo-bundle]: https://github.com/burtonageo/cargo-bundle

## Configuration

Settings are stored in `~/.config/ngai_tunes/config.json`.

```json
{
  "info_seconds": 5,
  "warning_seconds": 5,
  "error_seconds": 10,
  "repeat": "Off"
}
```

Omitted values use the defaults shown above. Notification durations are in
seconds. Warnings appear in yellow and errors in red. Invalid or unreadable
configuration falls back to defaults and prints an error on startup.

The daemon saves changes to `repeat` and restores the setting on startup.
The supported values are `Off`, `All`, and `One`.
