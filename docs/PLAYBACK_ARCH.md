# Playback implementation guide

Jolteon plays local MP3 and FLAC files through the same Rust code on Linux and macOS. For a player with a different UI or playback policy, reuse the decoder-to-output pipeline and keep track selection, queueing, and repeat behavior in the application layer.

## Dependencies and platform setup

| Library | Role |
| --- | --- |
| Rodio 0.22.2 | Opens the default output stream, mixes sources, and wraps each source with playback controls and position tracking. |
| Symphonia | Probes and decodes MP3 and FLAC into PCM through Rodio's `Decoder`. |
| CPAL 0.17.3 | Rodio's audio device layer in this checkout's `Cargo.lock`. It uses ALSA on Linux and CoreAudio on macOS. |
| Lofty 0.25.1 | Reads tags and duration for the UI when building a `Song`. |

A new project that only needs local MP3 and FLAC can start with this subset of [Jolteon's Cargo configuration](Cargo.toml). Jolteon also retains Rodio's defaults and enables extra formats.

```toml
[dependencies]
rodio = { version = "0.22.2", default-features = false, features = ["playback", "symphonia-mp3", "symphonia-flac"] }
```

On Linux, install the ALSA development package (`libasound2-dev` on Debian/Ubuntu or `alsa-lib` on Arch) and make `pkg-config` available for the build. On macOS, CPAL uses the system CoreAudio frameworks. Both platforms need an available default output device at runtime. The [CI matrix](.github/workflows/ci.yml) covers builds and tests for x86-64 Linux and Intel and Apple Silicon macOS. Audible playback requires a device test.

## From a file to the speakers

1. [Song](src/structs/song.rs) reads the file with Lofty to obtain duration and fields such as title, artist, album, and track number. Title falls back to the filename. Ordinary files start at zero. CUE entries carry an offset and a segment length within a shared file.
2. [SingleTrackPlayer](src/player.rs) owns the output stream on a worker thread. It receives commands through a standard-library channel and opens the default device with `DeviceSinkBuilder::from_default_device()?.open_stream()?` (the current code unwraps both operations). Keep the stream alive for the entire playback session.
3. [Source](src/source.rs) opens the file with `File::open` and uses `Decoder::try_from(file)` for both formats. Rodio pulls decoded `f32` PCM samples. The source applies these wrappers from innermost to outermost: `Speed` (1.0), `TrackPosition`, `Pausable`, `Amplify`, `Skippable`, `Stoppable`, and `PeriodicAccess`. Chiptune files use a separate Game Music Emu source behind the same interface.
4. For a CUE segment, the worker seeks to the file offset before calling `output_stream.mixer().add(source)`. CPAL's output thread then consumes the source. The nominal 5 ms `PeriodicAccess` hook publishes the sample-derived file position and applies pending pause, gain, seek, or stop requests. The worker uses the segment length and shared position to decide when it should stop.

## What the UI can read and control

| Need | Current interface |
| --- | --- |
| Now playing | `MainPlayer::playing_song() -> Option<Song>` returns a copy of the track metadata and length, or `None` when idle. |
| Progress and pause | `playing_position() -> Duration` reports sample-derived elapsed time relative to the track's start, without compensating for output buffering. `is_paused() -> bool` reports the pause flag. |
| Volume and repeat | `volume() -> u32` reports percentage and `repeat_mode()` reports `Off`, `One`, or `Queue`. Gain is applied in software. |
| Queue and notifications | `queue().with_items(...)` reads items. `on_queue_changed` and `on_error` register callbacks. `SingleTrackPlayer::on_playback_end` is consumed by the queue worker. |
| Commands | `MainPlayer::play(song)` and `stop()` delegate to the track worker. `single_track_player()` exposes pause/resume, relative seek in seconds, and volume controls. `seek_forward()` and `seek_backward()` step by five seconds. |

[MainPlayer](src/main_player.rs) handles queue advancement and repeat. The [TUI loop](src/app.rs) uses a 100 ms tick, uses the queue callback to flag a refresh, and currently logs playback errors. The [status widget](src/components/root/widget.rs) reads the playback getters when rendering. Linux can additionally use [MPRIS](src/mpris.rs) for desktop media integration.

## Reusing the design

- Put device and source ownership behind a UI-independent playback API. Keep one actor responsible for track changes and let your queue or other playback policy submit commands to it. Keep filesystem and tag reads outside the audio callback, and keep synchronization there short.
- Give the UI a consistent snapshot with track ID, metadata, elapsed time, duration, status, and gain. Keep start offsets as `Duration` throughout. Jolteon reads fields separately, truncates the UI start offset to whole seconds, and updates seek position optimistically before the decoder confirms it. Clamp seeks to the track range and report the resulting position or error.
- Distinguish natural EOF, user stop, replacement, and failure in events. Jolteon's source signals on iterator exhaustion, while the worker waits on commands and a length-based timeout before waiting for that signal. This can delay advancement if the file ends earlier than its reported length, and the wait for the signal can block if the device stops consuming the source. Its public end callback also runs after stop or file/decoder setup failure. A new player can wake the worker directly on EOF and use a track ID to ignore stale events. For CUE, enforce segment boundaries separately from file EOF.
- Return device startup errors to the UI and forward runtime stream errors if the UI needs to display them. Exercise MP3 and FLAC on both platforms, including pause/resume, seeking near EOF, replacement, missing or corrupt files, and playback with no default output device.
