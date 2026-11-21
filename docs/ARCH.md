# Architecture

[AGENTS.md](../AGENTS.md) defines the product behavior. Keep these boundaries
when choosing crates or changing the thread model.

## Processes and ownership

Define the CLI once with Rust’s `clap::Parser` derive API. Declare the
optional directory as `Option<PathBuf>` and mode flags as mutually exclusive
with each other and the directory. Let clap generate help, usage and parse
errors and handle the `--` end-of-options marker.

Dispatch an empty argument list and the `--attach` flag to the same
attach client. A directory argument selects the scan client, with `.`
resolved against the client’s working directory.
Support a foreground `--daemon` flag for debugging and a private detached child
mode for the launcher. Keep CLI parsing separate from IPC types.

| Module | Owns |
| --- | --- |
| client | Directory scan, metadata, displayed list, cursor, focus, cached session snapshot and terminal restoration |
| daemon | Singleton, listener, client connections, ordered commands, queue, playback order and shutdown |
| playback | One active audio stream, decoder, actual position and playback completion/error events |
| protocol | Serializable requests, responses, events and snapshots independent of TUI and audio crates |

The scan client resolves paths before sending them, since the daemon may
have started in another directory. Keep list order deterministic. An attach
client mirrors queue updates. A scan client updates its Playback panel
without overwriting its local list. On disconnect retain the last list,
clear live playback indicators and allow Main Enter to reconnect.

## Startup and shutdown

1. Try the socket. On Main Enter with no connection, spawn the same binary
   without a shell. Detach the child from the terminal with setsid and
   redirect standard streams. Report readiness or startup failure through
   a bounded handshake.
2. Use a private runtime directory (0700). Verify ownership and reject
   symlinks or unsafe existing paths. Hold an exclusive nonblocking flock
   on daemon.lock for the daemon lifetime. Only the lock holder may remove
   a stale socket and bind the new one.
3. A competing launcher waits a bounded time for the winner to accept
   connections. Report timeout to the client. Socket connection failure
   alone never grants permission to unlink an endpoint.
4. Serialize shutdown with controller commands. Stop accepting clients,
   close streams and unlink the socket before releasing the lock. Leave the
   lock file in place so concurrent processes lock the same inode.
5. Explicit quit is one controller operation. Client EOF releases only the
   connection. Evaluate exit_on_stopping when received, irrespective of the
   number of attached clients. Natural queue exhaustion exits independently.

## IPC and controller

Use newline-delimited UTF-8 JSON. Each request has a version, request_id
and command. Reply with the same ID and success or an error code/message.
Handle partial lines, multiple lines per read, unsupported versions and
malformed input. Enforce the 1 MiB frame limit on both ends.

- Main Enter uploads the ordered paths and metadata in numbered segments
  with the same request ID. Each segment includes the total segment count
  and its one-based index. Pack up to 100 tracks per segment and shrink
  segments as needed to fit the frame limit. Only the first segment
  includes the selected index in the full queue.
- Stage uploads per connection. Acknowledge a successful replacement after
  all segments arrive and the full queue is validated. Invalid or interrupted
  uploads leave the active queue in place.
- Provide subscribe/get-state, pause/resume, stop, restart, seek,
  next/previous, set-repeat, set-order, quit and exit_on_stopping.
- Subscribe before obtaining a snapshot. Send the queue in numbered
  segments when a connection first sees its queue revision. Send the snapshot
  after those segments with the queue revision, a monotonic state revision,
  current entry, status, duration if known, confirmed position and playback
  modes. The client assembles the complete queue before applying the snapshot.
  Later state changes reuse the cached queue for that revision. Reset both
  revision baselines on reconnect.
- Animate progress locally between updates while Playing. Resync on seek,
  pause and track change. Use bounded per-client output queues so a slow
  reader cannot stall playback. On connection loss after sending a command,
  refresh state before letting the user retry.

## Audio

Keep queue policy in one daemon controller. Behind a shared Rust backend
interface, open MP3/FLAC, play, pause, resume, stop, seek and report position.
Evaluate Rodio with Symphonia decoding on both systems before committing to
platform-specific code. Read metadata in the client and keep filesystem and
socket operations outside the audio callback.

Distinguish natural EOF, replacement, manual stop, track failure and device
failure. Tag each playback attempt with a generation so late EOF cannot
advance a replacement track. Clamp seek and publish the confirmed result.
Bound failed track attempts across the queue even under repeat/random.
Report device failure separately and retain the selection for retry.

## TUI

The diagram shows the arrangement of the panels. Their dimensions can
vary with the terminal size.

```text
+--------+--------------------------------------+
|        |                                      |
|        |                                      |
|        |                                      |
|        |                                      |
|  Side  |              Main                    |
|  Bar   |                                      |
|        |                                      |
|        |                                      |
|        |                                      |
|        |                                      |
|        |                                      |
|        |                                      |
+--------+--------------------------------------+
|                                               |
|                    Playback                   |
|                                               |
+-----------------------------------------------+
|                      Info                     |
+-----------------------------------------------+
|                     Hotkey                    |
+-----------------------------------------------+
```

| Region | What it shows | Interaction |
| --- | --- | --- |
| Sidebar | Available tabs and the active tab. A directory argument opens Browsing. | Selecting a tab chooses the view in Main. |
| Main | Browsing shows the local track list. Playing shows the daemon queue. Mark the cursor and the playing track independently whenever both are visible. | Up/Down moves the cursor locally. Enter updates Playing with the displayed list before sending it to the daemon to play from the selected entry. Space controls the live session even if the cursor is on another track. |
| Playback | The daemon's current track, status, elapsed time, a progress bar when duration is known, repeat mode and playback order. Clear live indicators on disconnect. | Space pauses or resumes. Enter stops or restarts the retained track. `n` selects next, `N` or `p` selects previous, and R cycles repeat. Seek and order controls also belong here. |
| Info | Status messages and errors from scanning, connecting or playing audio. | Display only. |
| Hotkey | Keys available in the current focus or modal, with their actions. For example, Enter means play selection in Main and stop in Playback while playing. | Display only. |

Tab switches focus between Main and Playback. Uppercase `T` focuses the
Sidebar, where Tab returns focus to Main. In Scanned, Tab cycles focus from
the song list through the filters and Playback.
Route Esc and q according to the [SPEC](../AGENTS.md). Restore terminal
modes on exit and error.
