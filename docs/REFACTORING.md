# Refactoring guide

The five proposals from commit `d53bd4e` have landed. Rendering lives in
`ui/render.rs`, keyboard routing has focus-specific handlers, library filters
use `Facet`, the daemon finishes uploads in `complete_upload`, and playback
traversal has its own type. The following proposals build on those boundaries.
Each numbered item can be a separate patch.

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
