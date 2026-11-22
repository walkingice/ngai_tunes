# Architecture

The binary runs as a terminal client or as a playback daemon. Multiple clients
can share the daemon. Each client owns its terminal, displayed lists, filters
and cursors. The daemon owns the live queue, playback modes and audio device.
See the [user-visible contract](../AGENTS.md) and [UI conventions](STYLE.md).

## From launch to playback

1. [main.rs](../src/main.rs) uses clap to dispatch a directory, the default
   attach mode, or the foreground daemon. The private `--detached-child`
   flag runs the same daemon entry point.
2. The [TUI entry point](../src/client/tui/mod.rs) reads direct children of a directory
   argument before opening the terminal. Both client modes try to connect to
   the existing daemon. A successful connection subscribes immediately.
3. In the [TUI module](../src/client/tui/mod.rs), `App::songs` chooses among the directory
   list, filtered library and local copy of the Playing queue. Enter on a
   song sends the whole displayed list and its selected index. An empty
   list returns before any connection attempt.
4. [ipc.rs](../src/client/ipc.rs) opens a fresh connection for this upload.
   If connection fails, it launches the same executable with null standard
   streams and a new session via `setsid`. The client retries the socket
   for up to two seconds. A reader thread assembles daemon messages and
   passes them to the UI through a bounded channel.
5. [runtime.rs](../src/daemon/runtime.rs) checks the runtime directory and
   takes an exclusive nonblocking file lock before binding the Unix socket
   under `/tmp/ngai_tunes`. The lock holder cleans up the socket on exit.
   A client disconnect releases its connection while playback continues.

## Queue transfer and command ordering

[protocol.rs](../src/protocol.rs) defines version 2 JSON Lines requests,
responses and state snapshots. Requests carry a request ID. Replies echo the
ID, while subscription updates arrive unsolicited. The frame limit is 1 MiB.

- The client translates the local `ReplaceAndPlay` command into ordered
  `UploadQueue` segments carrying one request ID. The first segment carries
  the selected index in the full queue. The segment encoder packs up to 100
  tracks per frame and reduces the count to fit the byte limit.
- The [daemon loop](../src/daemon/mod.rs) accepts peers and serializes their
  requests with controller ticks. Each peer has a reader, a writer, a bounded
  output channel and its own upload assembly state. The controller receives
  the replacement only after the complete queue has arrived.
- Queue revision and state revision serve different purposes. The daemon
  caches encoded queue frames and sends them before a snapshot when the
  recipient needs that queue revision. Position, status or mode updates can
  then reference the same queue. A new connection gets its own assembly state.

## Playback

The [controller](../src/daemon/controller.rs) applies transport commands,
chooses the next index, handles exhaustion and reports snapshots. Track
failures trigger bounded attempts to find a playable entry. A device failure
leaves a stopped selection available for retry. Generation numbers associate
end events with the playback attempt that produced them.

[playback.rs](../src/playback.rs) defines the `Engine` interface used by the
controller and its test doubles. The production `Audio` implementation uses
Rodio's decoder, output stream and sink. It checks for a first sample before
starting a track, reads position from the sink and reports device errors
through a channel. The client estimates elapsed time between snapshots.

## Library and persistence

- [scan.rs](../src/client/scan.rs) handles directory entries and tag reading.
  The positional directory feeds Browsing. A separate
  [recursive worker](../src/client/recursive_scan.rs) expands configured
  roots, checks for cancellation while walking, tracks its current directory
  and deduplicates file paths.
- [database.rs](../src/storage/database.rs) stores one `ScanTrack` per line
  of `database.jsonl`, preserving multi-value artists and genres. A scan
  writes a temporary file beside the database. Publishing replaces the
  database atomically. Dropping an unfinished writer discards its results.
- [library.rs](../src/client/library.rs) loads the database on a background
  thread and builds the Genres, Artists and Albums filters and song list.
  The UI polls for scan and library results alongside socket and key events.
- [config.rs](../src/storage/config.rs) stores repeat mode under `server` and scan
  roots, notice durations, and Scanned positions under `tui` in
  `~/.config/ngai_tunes/config.json`, beside the database.
  Writers take a file lock and replace the config by rename. Client saves
  check for competing edits and preserve the daemon's latest repeat setting.

## Configuration

Settings are stored in `~/.config/ngai_tunes/config.json`. The `server`
section stores the repeat mode. The `tui` section stores notice durations,
scan directories, and Scanned cursor positions. To migrate an older
configuration, move each top-level setting into its corresponding section.

```json
{
  "server": {
    "repeat": "Off"
  },
  "tui": {
    "info_seconds": 5,
    "warning_seconds": 5,
    "error_seconds": 10,
    "scan_directories": []
  }
}
```

Omitted fields use their defaults. Notice durations are measured in seconds.
Warnings appear in yellow and errors in red. If the configuration is invalid
or unreadable, the program uses defaults and prints an error at startup.

The daemon saves changes to `server.repeat` and restores the setting at startup.
Supported values are `Off`, `All`, and `One`.
