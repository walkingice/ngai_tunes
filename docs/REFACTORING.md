# Refactoring guide

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
