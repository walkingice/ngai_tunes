# Architecture

## Process and state ownership

A single Rust binary runs as a TUI client or a playback daemon.
Each client owns its terminal, library views, filters, and cursors. The
daemon owns the live queue, current track, transport state, and audio device.
An exclusive file lock enforces one daemon at the local socket. Its lifetime
is independent of client connections and OS service managers.

## Client and daemon communication

Clients send commands and subscribe to authoritative state updates using
JSON Lines over a Unix domain socket. The daemon serializes client commands
with playback events. It assembles queue uploads per connection and applies
a replacement only after the entire queue arrives. Separate queue and state
revisions let clients associate playback updates with the correct queue
without transferring it on every update.

## Playback

The daemon controller owns playback order, repeat policy, and recovery from
track failures. It associates engine events with the playback attempt that
produced them so a late event cannot advance a newer track. A shared audio
engine interface isolates decoding and device access from controller policy
and supports the same controller on Linux and macOS.

## Library and persistence

Directory browsing and recursive library scans belong to the client.
Background workers scan and load the saved library. A completed scan
publishes the new database atomically so readers see a complete library.

The daemon persists the queue and last successfully started track. Client
preferences and daemon settings share a configuration file. Writers lock
and atomically replace it while preserving settings owned by the other side.
