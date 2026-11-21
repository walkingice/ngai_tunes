# Refactoring guide

## Make the daemon's final reply explicit

When a client quits, the daemon needs to give the socket writer time to
send the reply before dropping the peer. Following this sequence currently
requires reading `respond`, the writer loop in `connect_peer`, and
`Peer::drop`. Each outgoing message carries both a `close` flag and an
optional `delivered` sender. A quit reply requires both. An oversized request in
`read_requests` closes the connection after its error reply without waking
a waiter. Ordinary
replies currently allocate a delivery channel that they immediately discard.
See [daemon message handling](../src/daemon/mod.rs).

1. Replace the two fields on `Output` with a private enum describing the three
   cases: continue writing, close after writing, and close after writing
   with a delivery notification. Create the channel in `respond` only when the
   controller requests shutdown. Keep the receiver at the call site.
2. In the writer, send the queued frames and then the message frame.
   After a successful final write, shut down the write half, notify any
   waiting handler and leave the loop. A failed write must still close
   the socket without signalling success.
3. Keep the existing 100 ms wait after enqueueing a final reply.
   Mark the peer graceful only when the writer confirms delivery.
   Enqueue failure, timeout and writer failure must retain the existing
   socket cleanup behavior.

Use `two_clients_share_state_and_quit_is_acknowledged` to check that the
quitting client receives its reply. Also exercise a writer failure with a
closed socket pair and confirm that the waiting handler receives no success
notification. Check that an oversized frame still receives its error before
EOF, while malformed JSON permits another request. This change is internal to the daemon's output path.

## Keep a queue upload and its selected track together

When the client uploads a queue, only the first segment carries the index
of the song to play. The daemon remembers that index until all tracks
arrive. Today `Peer::upload` keeps the index in `Peer::selection`, while
`QueueTransfer` owns the partial track list. If `QueueTransfer::push`
rejects an out-of-order segment, it drops the partial tracks and the
early return leaves the index behind. The next valid first segment
overwrites that index. The maintenance cost is having to trace both
lifetimes to see why the old index cannot be used.
See [Peer::upload](../src/daemon/mod.rs) and
[QueueTransfer::push](../src/protocol/segments.rs).

Give each peer one optional, private pending upload containing a
`QueueTransfer` and a selected index. Take it out of the peer when handling
a segment. A first segment with a selection starts a fresh upload, even
if an earlier upload was incomplete. A continuation requires the pending
upload and must omit the selection. Put the upload back only when
`push` accepts a segment and needs more tracks. On completion, return
the assembled tracks and their selection together. Keep sequence checks
in the existing shared assembler and preserve the existing error replies.

**Check** an interrupted upload followed by a new first segment plays
the newly selected song. After an invalid continuation, further
continuations must fail until a new first segment arrives. A single-segment
upload must still complete immediately, and a second client must see the
old queue until the daemon accepts the complete replacement.
