# ngai_tunes

ngai_tunes is a planned keyboard-driven MP3 and FLAC player for Linux and
macOS. Its TUI controls a local daemon so music can continue after the
terminal closes. The repository currently contains the design and
implementation plan. The commands below describe the planned executable.

## Build

Once the Rust crate is in place, run from the repository root with Cargo:

```sh
cargo build --release
```

## Usage

```sh
./target/release/ngai_tunes               # Scan the working directory
./target/release/ngai_tunes ~/Music/Jazz  # Scan one directory
./target/release/ngai_tunes attach        # Open the running queue
```

In the Main list, select a track with the arrow keys and press Enter to
start playback. Space pauses or resumes. Esc detaches and asks a manually stopped daemon to exit.
Press q and confirm to stop playback and exit. With repeat off, the daemon
exits when it reaches the end of the queue.

See the [SPEC](AGENTS.md), [architecture](docs/ARCH.md) and
[implementation plan](docs/PLANS.md).
