# Implementation plan

Start from [the SPEC](../AGENTS.md) and [the architecture](ARCH.md).
The initial repository contains design documents only. Complete one step
at a time. For each step, run the listed checks, review the diff and commit
the working increment. Record actual platform test results in the handoff.

## 1. Create the executable and scanner

Create a Cargo binary named ngai_tunes and client, daemon, playback and
protocol modules. Test CLI dispatch: no arguments and `attach` select attach mode,
`.` and other directory paths select scan mode, and `daemon` runs the server.
Scan only immediate regular music files, accepting MP3/FLAC extensions
case-insensitively. Sort by filename, extract tags and keep a playable entry
when tags are missing. Distinguish an unreadable directory from an empty one.

Check with a temporary directory containing tagged and untagged files,
a nested album and an unrelated file. Assert order, fallback title and
absolute paths. Run cargo test and cargo build --release.

## 2. Build the controller with a fake backend

Represent the three states, queue, current index, position and playback
generation explicitly. Implement replace-and-play, pause/resume, manual
stop, restart and sequential advancement. Inject a fake engine whose
position, EOF and failures tests can control.

Test pause/resume position, restart at zero, replacing a paused track,
stale EOF after replacement, skip missing files and final EOF requesting shutdown.
Check exit_on_stopping separately for all three states.

## 3. Add local IPC and foreground daemon

Implement typed messages and bounded framing. Implement lock acquisition,
bind, shutdown and stale socket cleanup in the order in ARCH.md. Inject a
temporary private runtime directory in tests. Serve the fake controller
in foreground mode first. Implement subscription and revisioned snapshots.

Use two real socket clients. Assert consistent state after interleaved
commands and that disconnecting either client leaves the engine running.
Test partial, malformed and oversized frames, a slow reader, lock contention,
and startup after killing the lock holder. Verify cleanup never removes
another running daemon's socket.

## 4. Implement lazy launch and reattachment

Add the detached launcher and readiness handshake. Startup scanning and
attach-only mode may probe the socket. Only a play request that needs a
daemon launches one. Recheck on every Main Enter. Keep the last displayed
list usable if the daemon has since exited.

Use subprocess test clients calling the launcher and IPC directly. Launch
two concurrently, trigger play in both, and assert that one
daemon wins. Exercise failed startup and timeout. Start from a terminal,
close it, and verify another client can attach to the same queue.

## 5. Integrate real playback on Linux and macOS

Implement the shared backend with one stream owner. Connect actual position,
pause, seek and track completion to the controller. Surface decoder errors
as track failures and output-device errors as recoverable playback errors.
Add small MP3 and FLAC fixtures with recorded redistribution rights.

Run decoder tests without a device. On each OS, play both formats through
an actual output, seek near EOF, pause/resume, replace a track and try an
unavailable device. Record which checks required hardware.

## 6. Wire the TUI

Render the regions and focus cycle in ARCH.md. Keep scan results separate
from session snapshots. Implement focus-specific Enter, Space, Esc, q and
the confirmation modal. Disable Main Enter on an empty list. Space still controls live playback. Preserve
the screen's list and cursor when the daemon exits.

With no daemon, verify a bare launch reports that and exits. Start Jazz,
detach, then run `ngai_tunes` from Rock and verify it shows Jazz.
Run `ngai_tunes .` from Rock and verify the Main list shows Rock. Verify Space controls Jazz
and Main Enter replaces it with Rock. Verify attach shows the live queue.
Exercise manual stop/restart, cancel/confirm quit and terminal restoration.

## 7. Complete playback modes and release checks

Implement seek controls, next/previous, repeat off/all/one and sequential
or random order. Before coding the traversal rules, record expected results
for manual next under repeat-one, previous in random order and exhaustion
of a random cycle. Use a seeded RNG in tests and bound failed attempts.

Test a single track, mixed valid/broken files, an entirely broken queue and
repeat-off exhaustion with the TUI open. Confirm Main Enter starts a fresh
daemon after exhaustion. Run cargo fmt --check, cargo clippy --all-targets
and cargo test on Linux and macOS. Verify README commands against the
release binary and update build prerequisites from the chosen backend.
