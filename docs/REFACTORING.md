# Refactoring guide

The five proposals from commit `d53bd4e` have landed. Rendering lives in
`ui/render.rs`, keyboard routing has focus-specific handlers, library filters
use `Facet`, the daemon finishes uploads in `complete_upload`, and playback
traversal has its own type. The following proposals build on those boundaries.
Each numbered item can be a separate patch. Start with item 1 for a small
change that can be checked directly in the TUI.

## 1. Describe Settings rows once

`App::main_key` counts selectable actions, `scan_activate` decodes the
cursor into an action, and `render_scan` converts it into a screen row.
Each directory has two actions and a blank spacer. With zero directories
there is an extra leading spacer. Adding a button currently requires
updating three different calculations. See [input](../src/client/ui.rs)
and [rendering](../src/client/ui/render.rs).

Define a private Settings row enum for Edit(index), Delete(index), Add,
Scan and Spacer. Generate the ordered rows from the directory list. Keep
the cursor indexed by selectable action. Navigation counts actionable
rows, activation matches the selected action, and drawing highlights its
position in the full sequence. Keep text and styling in the renderer.

**Check** an empty list offers Add and Scan. With two directories, move
through both Edit/Delete pairs, delete the last directory, and confirm
that Enter still acts on the highlighted row. Spacers must be skipped.

## 2. Let Filter own the meaning of the All row

`filter_key` counts one more row than there are values, `render_library`
prepends All, and `Library::update_songs` subtracts one from the applied
row to look up a tag. These three operations describe one indexing rule.
See [Filter](../src/client/library.rs), [input](../src/client/ui.rs) and
[rendering](../src/client/ui/render.rs).

Give Filter methods for the displayed labels, row count, and applied tag
value. Represent All as an absent applied value. A real tag named “All”
must still be selectable by its row. Use those methods in navigation,
drawing and matching. Keep the highlight cursor separate from the applied
row so moving the cursor alone does not rebuild Songs.

**Check** an empty facet still displays All. Enter on a secondary artist
matches tracks containing that artist, combining filters requires every
applied facet to match, and resetting restores all songs and cursor zero.

## 3. Make configuration load outcomes explicit

In [App](../src/client/ui.rs), `notice_config` holds the entire Config,
including scan targets. Whether it can be saved depends on `config_valid`
and a separate optional store. Reading `save_config` requires trusting
the invariant behind `expect("valid config has a store")`.

Use the existing Config name for the settings value. Group store access
and load status in a private enum representing three outcomes. A loaded
store permits saving and scanning. A store with a load error still gives
access to its database. An unavailable store has no database path. Build
the outcome in one place during initialization and keep the original error
for the notice. Keep displaying defaults after a load failure.

**Check** a malformed config beside an existing database still allows
opening Scanned. Saving targets and starting a scan must stay blocked in
that case. A successful load still saves through `save_if_unchanged`;
a failed save must keep both the old settings and the edit dialog.

## 4. Separate playback replies from snapshot updates

In [App::receive](../src/client/ui.rs), one message can acknowledge a
request, report a failure and carry a snapshot. The ordering matters.
For example, an old snapshot can arrive with a valid acknowledgement.
Also, a later command can replace `pending` while `pending_play` still
identifies the queue replacement awaiting a reply.

First extract request acknowledgement and error handling, keeping the
two request IDs distinct. Then extract snapshot application, including
the revision check, initial cursor choice, repeated-error suppression
and queue synchronization. Keep `receive` as the short ordered call site.
Leave socket reconnection in `poll_connection`.

**Check** a matching reply clears its request even with stale or absent
state. An unrelated snapshot during a pending upload leaves the displayed
replacement alone. A failed upload resynchronizes against the available
snapshot, and repeated snapshots with the same error do not renew the notice.

## 5. Isolate starting one playback candidate

In [Controller::try_tracks](../src/daemon/controller.rs), the same loop
updates the snapshot for a single candidate and decides how far to search
when tracks fail. The playback order is already encapsulated. The remaining
boundary is between attempting one track and choosing another.

Extract a private method taking a queue index and returning
`Result<(), StartError>`. Move the current index, position and duration
reset, generation increment, engine start and successful Playing transition
into it. Leave the initial stop, attempted-index tracking, error reporting
and shutdown decision in the outer method.

**Check** each attempt gets a fresh generation so an old end event cannot
advance the new track. A track error permits another candidate. A device
error ends the search with the daemon alive. An entirely broken queue
terminates even under repeat-all, without trying an index twice.
