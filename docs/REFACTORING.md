# Refactoring guide

These are proposed changes. Start with one numbered item per patch. Run
`cargo test` before and after each extraction. Keep wire formats, stored
files and keyboard behavior compatible throughout.

## 3. Name the library facets

In [library.rs](../src/client/library.rs), the facet labels, tag lookup
and filter array share a numeric ordering. The wildcard arm in tags
maps every unexpected index to Album. Introduce a Facet enum with an
ordered list, a label method and an exhaustive tag lookup. Convert to an
array index in one place. Pass a Facet from the UI when applying a filter.

Keep the highlighted row distinct from the applied selection. Check that
moving within Artists leaves Songs unchanged until Enter, that row zero
accepts all artists, and that changing a filter resets the song cursor.

## 4. Isolate queue upload completion

In [daemon/mod.rs](../src/daemon/mod.rs), handle_request validates
protocol versions, assembles uploads, persists repeat settings and runs
controller commands. Extract its UploadQueue branch into a function taking
the requesting peer, controller and queue frame cache. Keep version checking
before dispatch. Document why a pending segment yields no response.

Construct the replacement frames before changing playback. Update the cache
only after the controller accepts the completed upload. Test an incomplete
upload, an invalid selection and a successful replacement. Observe the queue
from a second client to check that partial uploads stay private.

## 5. Separate traversal policy from audio operations

In [controller.rs](../src/daemon/controller.rs), adjacent and reset_cycle
manage playback order while try_tracks opens audio and skips failures.
First move the ordering state and those two operations into a private
type. Leave the retry bound and Engine calls in Controller. Keep the
distinction between manual navigation and natural end in the caller.

Use the existing fake Engine tests to verify that Previous retraces random
history, repeat-one applies at natural end, stale end events are ignored,
and an entirely broken queue terminates even under repeat-all.
