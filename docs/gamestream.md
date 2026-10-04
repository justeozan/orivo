# Game streaming (Moonlight / Sunshine)

A game that runs on another machine and is played on this one. The machine that
runs it is a [Sunshine](https://github.com/LizardByte/Sunshine) host; the client
on this side is Moonlight. Orivo's part is a runner plugin —
`com.orivo.gamestream`, built in its own repository — plus the three host-side
pieces this document is about.

**Orivo never ships Moonlight, Sunshine, or a game.** The user installs the
client themselves and points a runner profile at it, exactly as they would at an
emulator.

## Why anything had to change on this side

The runner contract gives a plugin no network at all — no WASI, no socket, no
`network_fetch` import — and that is deliberate and permanent for this host. A
streaming plugin therefore cannot do the two things it would most obviously want
to: ask a remote machine what it can stream, and start a client with
`moonlight stream <host> <app>`. The v1 launch shape is one argument, the
resolved game file, and nothing else.

Both gaps are the host's to close, because both are about reaching another
machine and about building a process — the two things the sandbox exists to keep
away from guest code. So:

| Gap | Closed by | Where |
| --- | --- | --- |
| no network in the guest | the host asks the machine itself and writes the answer into the profile's folder | `src-tauri/src/gamestream.rs` |
| one-argument launch cannot start a stream | a second launch mode, `stream`, whose argument list the host builds from a document it wrote | `catalog.rs`, `plugin_runtime.rs`, `runner_host.rs` |
| nowhere to put the machine's address | one field in Settings › Plugins, shown once a profile streams | `src/gamestream-settings.ts` |
| the client has to be found and paired | detection of a standard install, and a PIN the host draws | `gamestream.rs`, `runner_commands.rs`, `runner-view.ts` |

The plugin's own account of the first two, and the trajectory they sit on, is in
its `docs/05-manques-et-plan.md` — §1.2 and §3.2 are what this implements.

## The client answers, not Sunshine's web API

`GET /api/apps` gives the same list, and it is what this did first. It asks for
the wrong thing. Sunshine's web API authenticates with the **admin account of
its web interface**, so Orivo would have to hold a password for a capability it
does not otherwise need, store it, and accept a self-signed certificate in order
to send it.

`moonlight list <host>` returns the same names and authenticates with the
**client certificate established at pairing** — the same authorisation that lets
the machine be streamed from at all. Which means:

- **no credential exists in this feature.** Nothing to ask for, store in clear,
  or keep out of the WebView. A user who can stream can list; a user who cannot
  list could not have streamed either.
- **nothing is specific to one host implementation.** It is Moonlight's own
  protocol, so Apollo and any other Sunshine fork answer it the same way.
- **a failure is actionable.** A refusal overwhelmingly means "not paired yet",
  and pairing is a button on the same card.

Two costs, both paid explicitly. The client binary has to be present — which it
has to be anyway, since it is also what plays the game — and **an unreachable
machine makes `list` wait rather than fail**, so every call is bounded by a
deadline and the child is killed when it passes. The reader thread is
deliberately not joined on that path: killing the client does not necessarily
close the pipe, because anything it spawned inherited the write end, so joining
would put the whole deadline back where it started.

## The `.stream` document

One file per streamable game, in a folder the user granted to the profile. The
name is the title; the body is the host's own document:

```json
{ "host": "astra.local", "client": "moonlight", "app": "Celeste" }
```

The extension is deliberately one nothing on the system opens: it must not be
associated with an application, and must not be filtered out as a type either.

**Orivo is the only writer of those bytes, and re-reads them at launch anyway.**
A plugin cannot write a file — it has no write capability — but a *user* can,
and so can anything else on the machine. So the launch does not trust the file
because the feed wrote it; it validates it again, every time:

- the body parses as exactly this document, with no extra fields, under 4 KiB;
- `client` is `moonlight` and nothing else;
- `host` is a host name, an IPv4 literal or a bracketed IPv6 literal, bounded,
  free of control characters, and not starting with `-`;
- `app` is bounded, free of control characters, and not starting with `-`.

There is no shell anywhere in this path — the arguments are passed as a vector,
never a command line — so the leading-`-` rule is not about quoting. It is about
refusing a string that *looks like* an option before it can become an argument.

## The launch

`RunnerLaunchMode` on a profile is the user's own permission, set in Settings
and never visible to `validate-profile`. The plugin's `prepare-launch` names a
mode too, per game; the host requires the two to agree, in both directions:

- a `stream` intent from a profile that only authorises game files → refused;
- a `default` intent from a profile that authorises streams → refused.

That is why the mode is an enum and not free text. A plugin that could name the
mode would, through it, be naming its own argument list. When both agree on
`stream`, the host reads the placeholder and builds:

```
<the profile's application> stream <host> <app>
```

Every element of which the host read and validated itself. Nothing the plugin
said reaches that vector.

## The feed

`GET {origin}/api/apps` against Sunshine's own web API, with Basic auth over
TLS, no redirects (a redirect would carry the `Authorization` header somewhere
it was not addressed), and a 4 MiB ceiling on the body. Sunshine's certificate
is self-signed out of the box and the address is one the user typed, so the
client accepts that certificate; there is no trust-on-first-use bookkeeping in
v1, and that is a known limitation rather than a decision to be proud of.

The answer becomes one `.stream` file per application, beside a manifest —
`.orivo-gamestream.json`, which the plugin's listing walks past — recording
which files the feed wrote. **That manifest is the whole authority for removal.**
A file it does not name is never rewritten and never deleted, which is what
makes the folder safe to share with files the user maintains themselves:

- a hand-made `Some Game.stream` survives every refresh;
- an application whose name collides with one yields rather than overwriting it;
- two applications that sanitise to the same filename get `Name.stream` and
  `Name (2).stream`, not one shared card;
- an application the host no longer lists has its placeholder removed — but only
  if the feed is the one that wrote it.

The refresh runs at the start of a stream profile's import, so the plugin walks
a folder that is already current. With no host configured the refresh is a
no-op rather than a failure: that is the manual mode the plugin documents, and
a hand-maintained folder keeps working. Any other feed error fails the import
carrying its own sentence, so "the machine is asleep" and "the password is
wrong" read differently in the panel.

## Pairing

Pairing is the client's own handshake with the machine, on the machine's own
ports. It is what authorises everything else — listing and streaming alike — and
it happens once per machine.

**The client chooses the PIN**, which is why Orivo can show it: the handshake has
the client commit to a PIN and the machine's operator confirm the same one. So a
stream profile's card offers *Pair with this host…*, which draws four digits,
starts `<the profile's application> pair <host> --pin <pin>`, and shows the
number until the user says they are done — not on a timer, because a PIN that
vanished while they were walking to the other machine is the one failure worth
avoiding. The address is the machine's, without any web port: pairing does not go
through the web interface.

**The state is read first, and that is the whole point of the shape.** A client
that is already paired *refuses to start a handshake*, so starting one anyway and
showing its PIN hands the user four digits the other machine can only reject —
which is exactly what the first version of this did. The state is read by
listing, because listing is precisely what pairing authorises: a machine that
answers with its games has accepted this client, and one that refuses has not.
That is cheaper to be right about than reading the client's own refusal, which is
a sentence in the user's language rather than a fact.

The PIN is rejection-sampled rather than reduced modulo 10000: 65536 is not a
multiple of it, so a plain `%` would make the first 5536 PINs likelier. It is a
short-lived shared secret for a handshake that authorises a client against a
machine, and skewing it costs nothing to avoid.

Sunshine also exposes the machine's half — `GET /api/pin` lists pending pairing
requests with a 32-hex `id`, and `POST /api/pin` takes
`{"pairing_id", "pin", "name"}` — so confirming the PIN without opening the web
interface is possible, and is the same action the web interface's own PIN box
performs. It is not done here for one reason: both of those calls need the admin
credentials this design exists to avoid, so an automatic flow would reintroduce
exactly what was removed. Apollo is a Sunshine fork and has not been checked
against this.

## Finding the client

A user who streams has the client installed, so making them walk a file dialog to
a bundle Orivo can see is asking them to do its work. Detection looks at a fixed
list of locations per platform — the `.app` bundles on macOS, the standard
install roots on Windows, the package and flatpak/snap launchers on Linux — and
offers each one it finds as *+ Add &lt;name&gt;* beside the picker, which stays,
because the list only covers where a normal install puts things.

Deliberately a fixed list rather than a `PATH` walk or a search: the answer
becomes a program the host starts, so what it can find is a decision made in that
function and reviewable there, not whatever a user's environment happens to point
at.

**The path never crosses IPC.** What a detected client offers the WebView is a
handle — the digest of its canonical path — and a name. A handle is only honoured
if it still matches something detection finds *now*, so the set of programs a
WebView can choose from is the set Orivo already decided to look for. That is the
same rule the rest of the runner surface follows, and it is load-bearing here: a
WebView that could hand back a path would be choosing which binary the host runs.

## One game, one card

A game the library already holds and that a machine can also stream is one game
to the person playing it. So an import of the second way to start it adds that
way to the card that is already there — with its artwork, its play time and its
place in every shelf — rather than a second entry beside it. That is the
`alternate_launch_targets` list on a game, and catalog schema v9 is what it
added.

Titles are compared with case and punctuation set aside, because the same game
is spelled differently by every shop that sells it: `Assassin's Creed - Unity`
has to find `Assassin's Creed Unity`. **Edition suffixes are deliberately left
alone**, so `Cyberpunk 2077` and `Cyberpunk 2077: Ultimate Edition` stay two
games — guessing that they are one is a merge the user cannot see and cannot
undo, and the cost of being wrong is a game disappearing into another's card.

Three properties fall out of doing it at import rather than once:

- **it heals.** A library that already carries the duplicate folds it in on the
  next import; nothing has to be removed by hand.
- **it does not stack.** An import runs again every time a library is
  refreshed, and the same way of starting the same game is recorded once.
- **it comes back.** Removing the profile takes its way of starting back out of
  every card, and leaves the cards that were never its own alone.

A non-runner card is preferred as the host when several titles match: it is the
one with the history, and the one that survives a runner profile being removed.

## Choosing, at the moment of playing

Nothing in Orivo is modal, so a card with two ways to start answers where it was
asked: a short list anchored to Play, not a window over the page. One way is
never a question — Play starts it. A way the host reports as unusable right now
is not offered at all.

A streamed game is never held back by the platform matrix. Two facts were being
confused: whether this game ships a build for this operating system, and whether
anything here can start it. A Windows-only game streamed from a Windows machine
is a game you can play, and printing "Windows only" over it is the wrong fact
about the right game. Only a way of starting that genuinely runs *elsewhere*
lifts the block — each option carries that as its own flag, deliberately
narrower than "launchable", because a store game counts as launchable whenever
its client is installed, which says nothing about whether a build exists for
this platform.

What crosses to the WebView is a handle derived from the launch target's own
content, never an index: the list is rebuilt on every render, and an index would
quietly mean something else if a profile were removed between the card being
drawn and Play being pressed. The target itself stays on this side; the host
resolves the handle against *that game's own* list, so a handle copied from
another card names nothing.

## Artwork

The cards an import creates arrive with nothing to show — for a streaming
library, that is a screen of grey tiles. The same search that fills a game's
covers on demand runs once the import has finished, for the cards it actually
created.

Three choices worth naming. It runs **after**, because an import that has not
finished does not yet know which cards exist. It skips cards it folded into
something the library already had, because those have their own artwork and an
import is not a reason to overwrite it. And a title no source has art for is a
normal answer that never stops the next card from getting its own.

Artwork from the streaming host itself was considered and rejected. It does not
come over the web API: it comes over the GameStream protocol, which only the
client speaks. What the client leaves on disk is a cache keyed by numeric
application id — ids `list` does not report — filled only as the user scrolls
its own grid, and indexed through its private settings file in a different
format on every platform. On the machine this was measured on it held three
covers out of forty-two.

## Settings

One address, under Plugins & Runners, and only once a profile actually launches
streams — an address for a machine nothing streams from is a box with no question
behind it.

There is no credential on this card, and no password anywhere in this feature.
A settings file written by the version that had one still loads; the extra fields
are dropped rather than failing the read, so an upgrade does not look like a lost
configuration.

A scheme and a port are accepted in the address and then discarded, because a
user who has been in Sunshine's web interface has that URL in their clipboard.
What reaches the client is the machine.

## What this is not, yet

- **No automatic PIN confirmation.** See above: the machine's side of pairing is
  still typed into Sunshine or Apollo.
- **No client installation.** Orivo finds a client that is already installed; it
  does not offer to install one. The matrix of "offer Moonlight / Artemis per
  platform" is host UI work that does not exist.
- **No network import in the guest.** Closing gap one in the plugin instead of
  the host would be a v2 contract revision — a WIT import, an allowlist, a
  raised SDK baseline — and is deliberately not done here. It is also no longer
  needed: the host does not reach the network either, it runs the client.
