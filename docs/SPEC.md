# Product specification

ngai_tunes is an English-language terminal music player for Linux and macOS.
One executable provides a client and a playback daemon for one local OS user.

## Sessions

Running `ngai-tunes` or `ngai-tunes --attach` connects to the current
session. Playing shows its queue and current track. Without a daemon,
the client opens with an empty queue and reports that no session is available.

Multiple clients can share playback. Closing a terminal leaves audio playing.
If the daemon exits, an open client retains its lists and selection.

## Finding and playing music

With a directory argument, Browsing lists MP3 and FLAC files directly inside
that directory. It reads titles from tags and falls back to filenames.
For a persistent library, users manage recursive scan targets in Settings.
A completed scan replaces the saved library. Cancelling preserves it.
Scanned lets users narrow the library by genre, artist, and album.

Playing a song replaces the session queue with the displayed song list and
starts the selected track from the beginning, launching a daemon if needed.
An empty list leaves the session untouched.

## Playback lifecycle

Users can pause and resume in place, seek, skip tracks, switch between
sequential and random order, and repeat one track or the whole queue.
Manual stop keeps the session and selection available for restart from zero.
On daemon startup, the saved queue opens stopped at the last successfully
started track, falling back to the first entry.

Users can close a client while playback continues or end a stopped session.
When playback runs its course, the daemon exits even with clients attached.
It skips unreadable tracks and exits if none are playable, even under repeat.
