# UI style guidelines

## Screen layout

Keep Tabs in a 13-column sidebar beside the current page. Playing, Browsing
and Scanned share the Songs table. On Scanned, place the Genres, Artists
and Albums filters in equal-width columns above Songs. Put Playback below
both the sidebar and the page so it stays visible when switching tabs.

Use the row below Playback for a right-aligned, temporary notice. Use the
terminal default foreground for information, yellow for warnings and red for
errors. Reserve the final row for dark gray key hints that reflect focus and
playback state.

## Focus and selection

Use a cyan border for the focused panel and dark gray borders elsewhere.
Prefix the active tab with `>` and bold its label while Tabs has focus.
In Songs, use black on cyan for the focused selection and white on dark gray
for the retained selection when focus moves away. Mark rows matching the
daemon's current track path with ♪ in a separate column so the cursor can
move independently.

## Dialogs

Center dialogs over the existing screen and clear their drawing area.
Leave one cell of padding between the border and content. Place dialog
buttons on the bottom content row, centered and bold.

The rendering helpers and terminal buffer tests live in [ui.rs](../src/client/ui.rs).
