# Refactoring guide

These are proposed changes. Start with one numbered item per patch. Run
`cargo test` before and after each extraction. Keep wire formats, stored
files and keyboard behavior compatible throughout.

## 5. Separate traversal policy from audio operations

In [controller.rs](../src/daemon/controller.rs), adjacent and reset_cycle
manage playback order while try_tracks opens audio and skips failures.
First move the ordering state and those two operations into a private
type. Leave the retry bound and Engine calls in Controller. Keep the
distinction between manual navigation and natural end in the caller.

Use the existing fake Engine tests to verify that Previous retraces random
history, repeat-one applies at natural end, stale end events are ignored,
and an entirely broken queue terminates even under repeat-all.
