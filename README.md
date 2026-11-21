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
To start a session, pass a directory. In the Main list, select a track with the arrow keys and press Enter to
start playback. Space pauses or resumes. Esc detaches and asks a manually stopped daemon to exit.
Press q to exit. If a track is playing, confirm to stop playback and exit.
With repeat off, the daemon exits when it reaches the end of the queue.

In Playing, Left/Right seek by five seconds while playing or paused.
Use Tab to focus Playback. Enter stops or restarts the retained track.
N/P move to the next or previous track, Left/Right seek by five seconds,
R cycles repeat, and O toggles sequential or random order. Repeat-one
applies to natural endings. Manual next and previous follow play order.
In random order, previous retraces the current shuffled cycle. Repeat-off
exits at the end of the cycle, and repeat-all starts a new shuffle.

See the [SPEC](AGENTS.md) and [architecture](docs/ARCH.md).
