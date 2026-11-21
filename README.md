# ngai_tunes

ngai_tunes is a keyboard-driven MP3 and FLAC player for Linux and
macOS. Its TUI controls a local daemon so music can continue after the
terminal closes.

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
```

If no daemon is running, launching without arguments or with `--attach` opens
an empty Playing queue and shows a message at the bottom.
To start a session, pass a directory. In the Browsing tab, select a track
with the arrow keys and press Enter to load the list into Playing and start
playback. Playing shows the daemon queue and starts empty without a daemon.
Space pauses or resumes. Esc detaches and asks a manually stopped daemon to exit.
Press q to exit. If a track is playing, confirm to stop playback and exit.
With repeat off, the daemon exits when it reaches the end of the queue.

In Playing, Left/Right seek by five seconds while playing or paused.
Tab switches focus between Main and Playback. Enter in Playback stops or
restarts the retained track.
Press uppercase `P` to focus Playback or uppercase `S` to focus the visible
song list.
`n` moves to the next track, and `N` or `p` moves to the previous track.
Left/Right seek by five seconds, R cycles repeat, and O toggles playback order. Repeat-one
applies to natural endings. Manual next and previous follow play order.
In random order, previous retraces the current shuffled cycle. Repeat-off
exits at the end of the cycle, and repeat-all starts a new shuffle.

Press uppercase `T` to focus Tabs, then Up/Down to select Settings and Enter
to open it. Press Tab from Tabs to return to Main.
Select a directory path to edit it, or select its Delete button to confirm
removal. Add directory creates a target. The Scan button asks for confirmation; scanning is not implemented yet.
Targets are saved in `config.json` under `scan_directories`.

In Scanned, Tab cycles from the song list through the filters and Playback.

See the [SPEC](AGENTS.md) and [architecture](docs/ARCH.md).

## Configuration

Optional notification durations are configured in
`~/.config/ngai_tunes/config.json`, in seconds:

```json
{
  "info_seconds": 5,
  "warning_seconds": 5,
  "error_seconds": 10
}
```

Omitted values use the defaults shown above. Warnings appear in yellow and
errors in red. Invalid or unreadable configuration falls back to defaults and
prints an error on startup.
