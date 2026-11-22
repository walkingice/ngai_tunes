# Menu client implementation plan

Users launch the menu client independently. It can stay open before, during
and after a TUI session. Detaching the TUI never starts the menu client.
Keep the existing CLI defaults, TUI shortcuts, queue ownership and daemon
lifetime rules. The menu has no library, file picker, queue editor, playback
timer or main window. All user-visible strings are English.

## 1. Add a compact IPC subscription

- Start in src/protocol.rs and src/daemon/mod.rs. Today Subscribe sends a
  Snapshot with a current index and separate queue frames. Add a distinct
  SubscribePlayback request and wire response carrying status, optional
  current title and artist, repeat, order, queue availability and any playback
  error needed by the menu. Use the controller's selected track even in
  Stopping. Keep position, paths and queue segments out of this response.
- Track the requested subscription per peer. Reply immediately with a summary
  and matching request ID. Push subsequent summaries when displayed fields
  change, including changes caused by other clients or natural playback
  transitions. Keep command acknowledgements and failures correlated by ID.
  Filter before scheduling writes so progress ticks do not wake menu peers.
- Preserve version 2 frames for existing commands and subscribers. Only send
  the new frame to peers that requested it. Surface an incompatible daemon
  as an error instead of repeatedly launching another process.
- Exercise both subscriber types on one daemon using the existing daemon test
  harness. Replace a large queue, change track and mode, and advance position.
  Verify the menu receives initial and changed summaries, zero queue segments
  and zero position-only notifications. Verify the TUI receives its full queue.

## 2. Share transport and daemon startup

- In src/client/ipc.rs, Connection::connect currently sends Subscribe and its
  reader feeds every frame to IncomingState. Factor socket opening, framing,
  request IDs and detached startup so menu connections can select their own
  subscription and decoder. Preserve the TUI entry points and their timing.
- Reuse the current_exe() launch with --detached-child, setsid, child reaping
  and bounded readiness retries. Reconnect before spawning and rely on the
  daemon lock for concurrent starts. Wait for the initial menu summary before
  choosing a playback command. Coalesce repeated Play clicks during startup.
- Run socket work off the UI thread. On startup connect once without spawning.
  On EOF discard stale controls and reconnect with increasing delays capped
  around 30 seconds. Play bypasses the delay. Cancel retries when exiting.
  Distinguish an absent socket or refused connection from permission,
  protocol and startup errors. Display actionable failures in the menu.
- Test offline startup with a fake connector that counts launches, including
  a daemon appearing between the offline observation and the Play click.

## 3. Implement the menu state and actions

Place the entry point and state-to-menu mapping in src/client/menu/mod.rs.
Declare the sibling module in src/client/mod.rs. Keep action mapping testable
without a desktop session. Apply authoritative daemon updates even while the
menu is open. Use the latest cached summary on opening. No one-second poll is
needed. Dispatch native UI mutations on the platform's event loop.

| State or action | Expected behavior |
| --- | --- |
| Offline | Show Server offline. Enable Play and Exit. Only Play may start a daemon. |
| Startup pending | Show Starting server and suppress duplicate playback requests. |
| Empty queue | Show Queue empty — add songs in the TUI. Disable playback and track navigation. |
| Stopping with selection | Play sends Restart and starts at zero. After offline Play, apply this rule to the restored queue. |
| Pausing | Play sends Resume at the existing position. |
| Playing | Pause sends Pause. If offline Play discovers an already playing daemon, leave playback running. |
| Track controls | Send Next or Previous. Let the daemon apply its navigation rules. |
| Repeat and Order | Send SetRepeat(Off/All/One) or SetOrder(Sequential/Random). Show exactly one checked value in each online submenu. |
| Exit | If cached offline, probe once in case the TUI started a daemon during backoff. If connected, send Quit and wait off-thread for daemon shutdown with a bounded timeout. On a lost reply or timeout, probe the socket. If still reachable, show an error and allow retry. If offline, close the menu. Never spawn during Exit. |

Show read-only title, then artist when present. Follow with Play or Pause,
Next, Previous, Repeat, Order and finally Exit. Disable mode changes offline.
Use a status or error row when needed. Test the table, including failed
commands, double clicks, empty restored queues, unexpected disconnects and
Exit during reconnect backoff after the TUI starts a daemon.

## 4. Wire the CLI and native menus

- Extend the clap parser in src/main.rs with --menu, mutually exclusive with
  directory, --attach, --daemon and --detached-child. Dispatch this mode to
  client::menu::run. Extend parser tests and retain the current default path.
- Use tray-icon and its menu re-export. Pin a release supporting KSNI
  (0.26.1 documents it). Use platform-specific Cargo dependencies. On Linux
  disable default features and enable ksni. Inspect cargo tree -e features
  for accidental GTK or AppIndicator activation.
- On macOS start the native main-thread event loop before creating the icon.
  Keep icon and menu handles alive and deliver IPC updates to that thread.
  On Linux use a StatusNotifierItem host and the KSNI worker. Route MenuEvent
  IDs to the shared actions and update existing item labels, checks and
  enabled flags. Verify updates while the native menu is already open.
- Test on macOS and a Linux desktop with a StatusNotifierItem host. Record
  how to identify a missing host in the Linux launch instructions.

## 5. Package and validate the user workflow

- Use cargo build --release for the CLI and cargo-bundle for the macOS .app.
  Configure the final Info.plist with LSUIElement true. Provide a bundle
  entry point that execs the real bundled ngai-tunes with --menu, so Finder
  launch and no-argument terminal launch select their respective clients.
  Resolve the executable relative to the bundle, independently of PATH and
  the working directory. Verify current_exe() in that process launches the
  real binary in detached-child mode. Include the tray icon in release builds.
- Run cargo fmt --check, cargo test and cargo clippy --all-targets. Exercise
  fresh and saved queues. Launch the app from Finder, check the Dock and
  menu bar, and test offline Play without a terminal open.
- Play from the TUI, detach with Esc, pause and resume from the menu, then
  reattach. Compare track, repeat and order from both clients. Let the queue
  finish naturally and verify the menu returns to offline. With both clients
  open, choose Exit and verify daemon shutdown, removal of the tray icon and
  continued TUI navigation. Repeat the interaction checks on Linux.
- Keep implementation commits focused on one verified increment. Update CLI
  help and project launch documentation when the mode ships.

References: [tray-icon platform requirements](https://docs.rs/tray-icon/latest/tray_icon/),
[cargo-bundle packaging](https://github.com/burtonageo/cargo-bundle).
