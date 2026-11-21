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
