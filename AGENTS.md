# ngai_tunes

## Project documentation

See the [UI style guidelines](docs/STYLE.md) for rendering conventions and the
[refactoring guide](docs/REFACTORING.md) for proposed maintenance work.

## SPEC

This file defines the user-visible contract for the first release. The
[architecture](docs/ARCH.md) describes the interfaces.

### Launch and attach

- Ship one Rust binary for Linux and macOS. Client mode is the default.
- With a directory argument, scan that directory. Use `ngai-tunes .` to
  scan the current working directory. List MP3 and FLAC files directly inside it
  in the Browsing tab. Read tags and fall back to the filename for the title.
- Opening the scan TUI checks for an existing daemon so the Playback panel can
  show the live session. Playing shows the daemon queue and starts empty if
  no daemon is running.
- Enter in the Songs panel checks for a daemon again, launches one if needed, replaces
  the Playing list with the displayed list, then sends that queue to the daemon
  and plays the focused entry from zero.
  An empty list has no playable selection and does not launch a daemon.
- With no arguments, `ngai-tunes` attaches to the existing daemon, loads its
  queue into Playing and selects the current track there. `ngai-tunes --attach`
  requests the same behavior. Open Scanned if the config directory contains
  `database.jsonl`, and Playing otherwise. If no daemon is reachable, leave the
  Playing queue empty and show a message at the bottom.
- Use Unix-style long flags for modes and a positional argument for the
  directory. Accept at most one of `--attach`, `--daemon` and a directory.
- Multiple clients may share one daemon. Closing a terminal leaves audio
  playing. A daemon exit leaves an open TUI usable with its list and cursor.
- The daemon saves changed Playing queues to `server_playing.jsonl` in the
  config directory and records the last successfully started track in
  `server_playing_cursor.json`. On startup it restores the queue in Stopping,
  selecting the saved track or the first track if the record is unavailable.
  Play in the menu restarts the selected track from zero.

### Library and navigation

- Playing shows the daemon queue. Browsing is available only when launched
  with a directory. Scanned reads the saved music library in the background.
- Settings lets users add, edit and delete recursive scan targets. Confirm
  Scan to rebuild the library. Cancelling a scan keeps the previous database.
  A successful scan replaces the database and reloads Scanned.
- Scanned has Genres, Artists and Albums filters. Move the highlight and press
  Enter to apply a filter. Each filter scopes the choices below it in
  Genres → Artists → Albums → Songs order. Unavailable selections return
  to All. R resets all three. Enter in Songs queues the filtered list.
- T focuses Tabs, S focuses Songs where available, and P focuses Playback.
  Tab cycles focus through the page controls and Playback. In Scanned, the
  order is Genres → Artists → Albums → Songs → Playback. Outside dialogs,
  h/j/k/l act as left/down/up/right. Dialogs handle their own input.

### Playback and exit

These shortcuts apply when a dialog is closed.

| Input or event | Result |
| --- | --- |
| Space outside Tabs | Pause Playing or resume Pausing at the same position. |
| Enter in Playback while playing or paused | Stop manually. Keep the daemon, queue and selected track in Stopping. |
| Enter in Playback while manually stopped | Restart the retained track from zero. |
| Esc | Send exit_on_stopping to an existing daemon and close the client. Only Stopping exits in response. |
| q | Stop a playing or paused track. In Stopping or without a daemon, close this client and terminate any running daemon. |
| Natural end with another track due | Advance according to the daemon's playback order and repeat mode. |
| Natural end with nothing left to play | Exit the daemon, even with a client attached. |
| Track missing or unreadable | Skip it. Exit if the entire queue is unplayable, including under repeat. |

In Playback, n advances, p or N goes back, r cycles repeat off/all/one,
and o switches sequential/random order. Left/right seek by five seconds in
Playback or in Songs while a track is playing or paused.
The client owns focus and selection. The daemon owns playback state.

### Runtime and scope

Target one OS user on the host. Use /tmp/ngai_tunes/daemon.sock and
/tmp/ngai_tunes/daemon.lock. Store persistent configuration, including scan targets, notice durations and repeat mode,
in ~/.config/ngai_tunes/config.json. Use flock and JSON Lines over a Unix domain socket. The daemon runs
independently of systemd and launchd. Use a shared playback interface and
implementation on both platforms, isolating platform differences as needed.

Tag editing, playlist-file import/export, native GUI clients and localization
are future work. The first interface uses English.
