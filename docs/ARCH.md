# Architecture

[AGENTS.md](../AGENTS.md) defines the product behavior. Keep these boundaries
when choosing crates or changing the thread model.

## Processes and ownership

Dispatch an empty argument list and the `attach` subcommand to the same
attach client. A directory argument selects the scan client, with `.`
resolved against the client’s working directory.
Support a foreground daemon mode for debugging and a private detached child
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
malformed input. Enforce a frame size limit on both ends and report an
oversized queue without partially replacing the current queue.

- Main Enter sends one replace-and-play request with ordered paths,
  metadata and the selected index. Validate the entire request before
  changing state. Serialize it with commands from other clients.
- Provide subscribe/get-state, pause/resume, stop, restart, seek,
  next/previous, set-repeat, set-order, quit and exit_on_stopping.
- Subscribe before obtaining a snapshot. Include a monotonic revision,
  queue entry identifiers, current entry, status, duration if known,
  confirmed position and playback modes. Discard older events within the same connection. Reset the revision
  baseline on reconnect.
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

Use a Sidebar (Playing tab), Main list, Playback panel with progress,
Info line for errors and contextual Hotkey line. Tab cycles focus between
Sidebar, Main and Playback. Distinguish the cursor from the playing entry.
Show the source directory or daemon queue as the list heading.

Arrow keys move the Main cursor. Enter depends on focus as defined in the
SPEC. In Playback, N/P select next/previous and R cycles repeat. Expose
seek and order controls and show their bindings in the Hotkey line. Route
q through confirmation. Restore terminal modes on exit and error.
