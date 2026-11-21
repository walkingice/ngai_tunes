# Music Daemon Architecture & Implementation Guide

> Implementation specification for a lightweight terminal music player targeting **Linux and macOS**. An AI coding agent should implement the design in this document and treat the acceptance criteria as requirements.

## 1. Goals and non-goals

### Goals

- Music continues playing after the TUI detaches or its terminal closes.
- A new TUI process can attach at any time to the same running playback session.
- At most **one daemon per user/application instance** is active (for the chosen runtime paths).
- No dependency on systemd, launchd, tmux, or any external background service.
- Minimal resource usage; keep the daemon independent of UI rendering and terminal state.
- Support Linux and macOS with a single Rust executable and subcommands.
- Allow multiple simultaneously connected clients, with one authoritative playback state.

### Non-goals

- Automatic restart, session recovery after crash, or persistence of playback state across daemon restarts.
- Surviving OS sleep, logout/session termination, reboot, or forced process termination.
- Network access or remote clients.
- A complex async runtime, database, or PTY emulation.

## 2. Processes and components

```text
Terminal A                       Terminal B
   |                                |
TUI Client A                    TUI Client B
   |                                |
   +-------- Unix socket ----------+
                    |
             Music Daemon
          +---------+---------+
          |         |         |
      IPC Server  Controller  Playlist/State
                    |
               Audio Engine
                    |
             OS audio output
```

The **daemon owns**:

- Audio decoding, output device integration, playback lifecycle (play/pause/stop/seek/next/previous).
- Playlist, current track, volume, progress and authoritative state.
- IPC listener, command processing, client notifications and per-client connections.
- Exclusive instance lock and its Unix socket endpoint.

The **TUI client owns**:

- Terminal setup/restore, keyboard handling, drawing, local UI state.
- IPC connection/reconnection and rendering from daemon state.
- **No audio device, playback state authority, or child daemon lifetime dependency.**

Use a clear Rust module boundary, e.g.:

```text
src/
  main.rs             # CLI dispatcher
  daemon/
    mod.rs            # daemon lifecycle and startup readiness
    singleton.rs      # exclusive lock, socket cleanup, runtime paths
    ipc.rs            # listener, client sessions, request dispatch
    controller.rs     # playback command handling
    state.rs          # playlist and authoritative playback state
    audio.rs          # audio backend abstraction
  client/
    mod.rs            # attach/detach workflow
    tui.rs            # terminal UI (ratatui/crossterm if used)
    ipc.rs            # protocol client
  protocol.rs         # shared request/response/event definitions
```

Modules may be adjusted to fit an existing repository; preserve the boundaries.

## 3. CLI contract

```sh
music                # Connect to daemon; spawn it if not running; enter TUI
music daemon         # Run daemon in foreground (useful for debugging)
music attach         # Attach only; return an error if daemon is absent
music quit           # Ask daemon to shut down gracefully
```

Inside the TUI, provide a **detach** command/keybinding that exits only the client. Closing a TUI or its terminal must not send `quit`, `stop`, or an audio shutdown request.

Optional CLI variants are acceptable, but clearly distinguish **detach**, **stop playback**, and **terminate daemon**.

## 4. Standalone daemon creation

For `music` when no daemon is available:

1. Resolve the per-user runtime directory and socket/lock paths (section 6).
2. Try to connect to the daemon; if connected, attach immediately.
3. Otherwise spawn the **same executable** with a private daemon child mode, with its stdio redirected to `/dev/null` or an explicit log file.
4. In the child, detach from the controlling terminal (Unix `setsid` via appropriate process setup) **before running the daemon body**. Do not rely on a terminal-owned foreground child surviving terminal closure.
5. Child acquires the singleton lock, clears a stale socket, binds IPC, and initializes the playback engine.
6. Signal *startup readiness* to the launching process via a dedicated pipe (success or failure), or another bounded, explicit handshake. Do not use unbounded sleeps/polling.
7. Parent attaches through the socket. The daemon keeps running if the TUI exits.

**Implementation notes**:

- When launching, avoid a shell wrapper and avoid inheriting the TUI's stdin/stdout/stderr or terminal-dependent handles.
- Prevent accidental daemon recursion; use a private child mode/flag for the spawned daemon.
- The explicit `music daemon` command can remain foreground for development, with appropriate error messages; it still follows the same singleton logic.
- Do not introduce double-forking unless a concrete OS requirement justifies it. `setsid`, redirected stdio, and a separate child process are sufficient for the intended terminal-close behavior in ordinary sessions.
- A supervisor, service manager or automatic respawn is intentionally out of scope.

## 5. Singleton guarantee: advisory lock + Unix socket

Use `flock()` on a dedicated lock file **and hold the lock file descriptor for the entire daemon lifetime**.

Startup algorithm:

```text
open/create music.lock (private permissions)
  -> acquire exclusive nonblocking flock
       -> lock busy: another daemon is starting/running; do not spawn another
       -> lock acquired:
            remove stale music.sock *only while holding lock*
            bind music.sock
            set private socket permissions
            start IPC server and audio engine
            hold lock until shutdown
```

Important correctness constraints:

- Never infer daemon absence solely from `connect()` failure; a daemon may still be starting.
- Never unlink a socket path unless holding the exclusive instance lock. Otherwise a competing process could unlink a live daemon's socket.
- A lock file may remain after a crash. It is harmless: kernel releases the **lock** when relevant descriptors close. **Do not unlink the lock file** during normal operation, to avoid two processes locking different inodes.
- On lock contention, the *launcher* should wait for a bounded amount of time for the existing daemon to become reachable, then provide a useful error if it never becomes ready.
- If bind/startup fails, release the lock on process exit and report failure. After acquiring the lock, cleanup of a stale socket is safe for this application's own private endpoint.
- On graceful shutdown, close clients/listener, unlink the socket **while still holding the lock**, then release the lock; ensure no other daemon can bind in the middle.
- The chosen lock provides single-instance protection only for cooperating processes that use the same path. Document any alternate `--runtime-dir` or application instance namespace.

Use `flock` supported on Linux/macOS; a small Unix-specific dependency (`libc`/`nix`/`rustix`) is acceptable.

## 6. Runtime location, permissions and security

Use a per-user runtime directory with mode `0700` and files under it:

```text
<runtime-dir>/music.lock
<runtime-dir>/music.sock
```

- Linux: prefer `$XDG_RUNTIME_DIR/<app-name>/`; validate ownership/security rather than trusting a user-supplied environment path blindly.
- macOS: use an app-specific private directory under a suitable user-owned location (e.g. `$TMPDIR/<app-name>/` with strict permissions); do not use a globally shared, predictable `/tmp/music.sock`.
- When creating private directories/files, avoid symlink traversal and permission races; verify ownership and permissions where appropriate.
- Socket protocol is local-only. Set socket permissions (`0600` where supported) and ensure parent directory is private.
- The client must derive *exactly the same* paths as the daemon.

## 7. IPC protocol

Use **Unix Domain Stream Sockets** (`UnixListener`, `UnixStream`) for Linux/macOS. TCP, network ports, and shared-memory IPC are unnecessary.

Initial version: newline-delimited UTF-8 JSON (`JSON Lines`), one complete message per line. Define typed serde messages with explicit `type`, `request_id` and `version` fields as appropriate.

Example request:

```json
{"type":"request","version":1,"request_id":42,"command":"pause"}
```

Example response:

```json
{"type":"response","version":1,"request_id":42,"ok":true}
```

Example state notification:

```json
{"type":"event","version":1,"event":"playback_changed","state":{"status":"paused","track_id":"song-1","position_ms":42000}}
```

Minimum commands:

- `get_state`, `get_playlist`, `play`, `pause`, `stop`, `seek`, `next`, `previous`, `set_volume`, `quit`.
- Add queue manipulation only when the playback implementation needs it.

Rules:

- Respond to every request with matching `request_id`, success/failure and a useful error code/message.
- Handle partial reads, EOF, multiple frames per read, malformed JSON, and bounded message sizes (e.g. 1 MiB max).
- A client can subscribe to events; on attach it must fetch an authoritative snapshot and then receive state-change notifications. Design the snapshot/subscription handshake to avoid missed updates (e.g. subscribe first, then snapshot with a monotonically increasing revision; discard older events).
- Do not broadcast high-frequency per-audio-buffer messages. Send meaningful state changes; clients can animate the position using reported position, timestamp and playback status, then resync periodically.
- IPC failure or client disconnection **must not interrupt audio playback**.
- Keep protocol structs independent of TUI and audio backend libraries.

## 8. Concurrency and audio safety

Keep an intentionally small concurrency model:

- IPC accept/handling loop and per-client handling as needed (standard threads are sufficient).
- One authoritative playback controller / command-processing context.
- Audio output handled by the backend's callback or dedicated output thread.

**Do not perform blocking socket I/O, expensive decoding, disk I/O, or wait on contended locks inside a real-time audio callback.** Use bounded buffers and/or message passing between the controller/decoder and audio callback. Prevent a slow client from blocking the controller: use bounded outgoing event queues; disconnect or coalesce updates for slow clients.

Tokio is optional, not a requirement. Avoid adding a runtime solely for a few local IPC clients.

## 9. Playback integration

Audio backend is replaceable and owned exclusively by the daemon. Candidate Rust choices:

- `miniaudio` bindings: cross-platform playback and decoding where supported.
- `cpal` for output and `symphonia` for decoding, if modular control is preferred.

Select one based on project needs. Do not instantiate the audio engine in a TUI client.

State should distinguish at least `stopped`, `playing`, `paused`, current track, position, duration when known, volume, and playlist. Handle end-of-track transitions and audio device failures explicitly.

## 10. OS limits and expected behavior

| Event | Expected result |
|---|---|
| TUI detaches | Audio continues |
| Terminal is closed | Audio continues while daemon remains alive and audio session is available |
| New terminal attaches | Same session and playback state |
| Multiple clients attach | All observe the same authoritative state |
| User logs out | **Not guaranteed** on either OS; Linux session cleanup or macOS user-agent/session behavior may terminate processes/audio access |
| OS sleeps / laptop lid closes | Playback generally stops while asleep |
| Audio device removed | Backend must report failure/change and recover if feasible |
| Daemon crashes | Playback stops; no automatic restart required |

Neither Linux nor macOS requires a visible TUI for normal background audio output. Avoid claiming that `setsid()` makes a process immune to logout cleanup, OS sleep, user-session shutdown, or forced termination.

## 11. Suggested implementation sequence

1. Define typed IPC protocol, paths, and CLI dispatch.
2. Implement exclusive `flock` ownership and socket lifecycle; verify two simultaneous launch attempts cannot create two daemons.
3. Implement foreground `music daemon` with a fake playback engine and `music attach` CLI commands.
4. Implement detached child spawning, readiness handshake, and parent attach.
5. Implement authoritative state/controller with pause/play/seek and event notifications.
6. Integrate audio backend and playlist.
7. Implement TUI and detach, terminal cleanup, connection recovery.
8. Test Linux/macOS behavior, crash/stale-socket cleanup and shutdown races.

## 12. Acceptance criteria / tests

- [ ] `music` starts exactly one daemon and opens the TUI when none exists.
- [ ] Launching `music` concurrently from multiple terminals yields **one daemon**, with clients attaching or receiving a clear bounded startup error.
- [ ] A detached TUI can exit completely; playback continues.
- [ ] Closing the terminal does not kill playback in a normal logged-in session.
- [ ] `music attach` from a new terminal renders the current playlist, track and playback position without restarting playback.
- [ ] Multiple TUI clients can control the same playback state consistently.
- [ ] Disconnecting/crashing any client does not stop the daemon.
- [ ] Killing the daemon releases the advisory lock; a subsequent launch safely removes the stale socket and starts normally.
- [ ] Graceful `quit` terminates playback, closes IPC, cleans up its socket, and releases the lock.
- [ ] Malformed/oversized IPC frames do not crash the daemon.
- [ ] Slow/disconnected clients do not block the audio callback or other clients.
- [ ] Linux and macOS builds do not require systemd/launchd integration.

## 13. Implementation constraints for coding agents

- Favor simple, testable code and minimal dependencies.
- Do not add automatic restart, database persistence, system service installation, or a network API unless explicitly requested.
- Do not use a PID file as the primary singleton mechanism.
- Do not emulate tmux or create a PTY for playback; the TUI is a replaceable client.
- Never let TUI teardown implicitly shut down the daemon or close its audio device.
- Make lifecycle and error cases explicit rather than relying on timing assumptions.
- Add unit/integration tests for singleton races, stale endpoints, IPC framing, and client detach.

### Reference documentation

- Rust Unix IPC: https://doc.rust-lang.org/std/os/unix/net/
- Linux `flock(2)`: https://man7.org/linux/man-pages/man2/flock.2.html
- Linux `unix(7)`: https://man7.org/linux/man-pages/man7/unix.7.html
- Linux `setsid(2)`: https://man7.org/linux/man-pages/man2/setsid.2.html
- Rust cpal: https://github.com/RustAudio/cpal
- Rust Symphonia: https://github.com/pdeljanov/Symphonia
- miniaudio: https://github.com/mackron/miniaudio
