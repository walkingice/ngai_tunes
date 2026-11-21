# ngai_tunes

## SPEC

This file defines the user-visible contract for the first release. The
[architecture](docs/ARCH.md) describes the interfaces and the
[implementation plan](docs/PLANS.md) gives the build order.

### Launch and attach

- Ship one Rust binary for Linux and macOS. Client mode is the default.
- With no arguments, scan the current working directory. With a directory
  argument, scan that directory. List MP3 and FLAC files directly inside it
  in the Playing tab. Read tags and fall back to the filename for the title.
- Opening the TUI checks for an existing daemon so the Playback panel can
  show the live session. The Main list always reflects the requested scan.
- Main Enter checks for a daemon again, launches one if needed, replaces
  its queue with the displayed list and plays the focused entry from zero.
  An empty list has no playable selection and does not launch a daemon.
- `ngai_tunes attach` shows the existing daemon queue and selects the current
  track initially. If no daemon is reachable, report that and exit.
- Multiple clients may share one daemon. Closing a terminal leaves audio
  playing. A daemon exit leaves an open TUI usable with its list and cursor.

### Playback and exit

| Input or event | Result |
| --- | --- |
| Space in Main or Playback | Pause Playing or resume Pausing at the same position. |
| Enter in Playback while playing or paused | Stop manually. Keep the daemon, queue and selected track in Stopping. |
| Enter in Playback while manually stopped | Restart the retained track from zero. |
| Esc | Send exit_on_stopping to an existing daemon and close the client. Only Stopping exits in response. |
| q, then confirm | Close this client and terminate any running daemon. Cancel returns to the TUI. |
| Natural end with another track due | Advance according to the daemon's playback order and repeat mode. |
| Natural end with nothing left to play | Exit the daemon, even with a client attached. |
| Track missing or unreadable | Skip it. Exit if the entire queue is unplayable, including under repeat. |

Support seek, previous/next, repeat off/all/one and sequential/random order.
The client owns focus and selection. The daemon owns playback state.

### Runtime and scope

Target one OS user on the host. Use /tmp/ngai_tunes/daemon.sock and
/tmp/ngai_tunes/daemon.lock. Reserve ~/.config/ngai_tunes/ for persistent
files. Use flock and JSON Lines over a Unix domain socket. The daemon runs
independently of systemd and launchd. Use a shared playback interface and
implementation on both platforms, isolating platform differences as needed.

Tag editing, playlist-file import/export, native GUI clients and localization
are future work. The first interface uses English.
