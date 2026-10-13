# Microsoft Agent (agentsvrs.exe + .ACS/.ACF characters) — behavioral spec

Reference model for bonzibuddy-rs walk/idle/notify semantics, speech-bubble timing,
and edge/taskbar interaction. Cross-referenced against `BONZI432_RE.md` §1–2 (Bonzi
walk + behavior scheduler).

**Provenance tags** — every claim is one of:

- **[B]** binary-verified in this corpus (static analysis only; nothing executed)
- **[D]** doc-sourced from the MSDN-era Microsoft Agent documentation, surviving
  today on learn.microsoft.com `/windows/win32/lwef/*` (pulled 2026-10-13 as the
  GitHub `MicrosoftDocs/win32` `desktop-src/lwef` mirror, 644 files)
- **[I]** inference from [B]/[D] evidence
- **[U]** undocumented gap — real behavior not recoverable from this corpus

Corpus (full manifest with sha256: `~/reference/bonzibuddy/msagent/manifest.tsv`):

| artifact | what | source |
|---|---|---|
| `msagent_202506_MSagent.exe` → `AGENTSVR.EXE` 2.0, `AGENTCTL.DLL`, `AGENTSR.DLL`, `AGTCTL15.TLB`, `AGT0409.HLP`, `MSLWVTTS.DLL` | MS Agent 2.0 runtime (server, ActiveX control, type lib, TTS glue) | IA item `msagent_202506` |
| `AgentSDK.iso` (volid `AgentSDK`) | official 1999 "Companion Disc for Developing for Microsoft Agent": full SDK, samples, `AGT0409.HLP`, and the four official characters | Wayback capture of the official download (exact URL not logged) |
| `x/chars/{Merlin,Genie,Peedy,Robby}/*.acs` | the four official v2.1 characters | AgentSDK.iso |
| `x/marvindocs/Marvin Help Documentation.chm` | Agent SDK help (Marvin character bundle) | IA item `marvin-help-documentation-file` |
| `lwef/` (644 md) | MSDN Agent docs mirror | GitHub MicrosoftDocs/win32 |
| `acs_parse.py`, `*_full.json` | structural .ACS parser + parsed dumps of all four characters | this work |

The .ACS parse is structural (magic `0xABCDABC3`; CHARACTERINFO / ANIM / IMAGE /
AUDIO sections), following Remy Lebeau's "MSAgent Character Data Specification"
with binary-verified corrections (branch count is u8; overlay record is 14 bytes;
voice-present flag is bit 5, not bit 4). Merlin's parsed state table matches the
official Merlin animation reference [D] row-for-row, which validates the parser.

---

## 1. Architecture and object model

- MS Agent is an **out-of-process COM server**: one `AGENTSVR.EXE` ("Microsoft
  Agent Server 2.0", FileVersion 2.0.x) per interactive desktop registers the
  character objects (`CoRegisterClassObject`/`CoRevokeClassObject` imports) [B].
  Clients talk to it through the `Agent.2` ActiveX control (`AGENTCTL.DLL`,
  type lib `AGTCTL15.TLB`) or the COM interfaces directly [B].
- The server owns real top-level windows: window classes `Agent` (the sprite),
  `AgentCharacterSendWindowClass`, `MainAgentSendWindowClass`,
  `AgentNotifySinkProxyWindowClass`, `AudioMgrWndClass` [B]. The sprite window is
  a transparent, region-shaped (`SetWindowRgn` import [B]), always-on-top window
  "on top of the desktop and all windows" [D].
- **Characters are shared across processes.** Any number of clients can load the
  same character; each client gets its own request queue, but the sprite, its
  position, and its visibility are global state [D]. One client at a time is the
  *input-active client* of the *topmost character*; `Activate(0|1|2)` manages
  active-client / topmost state; `HasOtherClients` lets an app detect sharing [D].
- **API surface** (verified against TLB + docs): `Load`, `Get`, `Prepare`, `Play`,
  `Speak`, `Think`, `MoveTo`, `Show`, `Hide`, `Stop`, `StopAll`, `Interrupt`,
  `Wait`, `GestureAt`, `Listen`, `SetPosition/GetPosition`, `SetSize/GetSize`,
  balloon/commands/property objects [B TLB, D].
  - **`MoveToRelative` does not exist in 2.0** — not in the TLB, not in the
    method list [B+D]. (It was an Agent 1.x control method; relative moves in
    2.0 are `GetPosition` + `MoveTo`.)
  - The TLB still carries the 1.x-era **interactive `Request`** object
    (`IAgentCtlRequest`: `Request`, `Button`, `Caption`, `InterruptRequest`,
    `WaitForRequest`) [B]: `Request(Number)` pops a balloon with predefined
    text and buttons; the request object's `Button` reports which button was
    clicked; `WaitForRequest` blocks; `InterruptRequest` cancels. Button-number →
    message table is 1.x help, not in this corpus [U].
- **Async request model** [D]: every queued method (`Load/Get/Play/Speak/MoveTo/
  Show/Hide/Interrupt/Wait/GestureAt`) returns a **Request** with `Status`:
  `0 completed, 1 failed, 2 pending, 3 interrupted, 4 in progress`, plus
  `Number`/`Description` error fields. `RequestStart`/`RequestComplete` events
  fire only for clients that kept a live reference to the Request object.
- **Event sink** (`IAgentNotifySink(Ex)`): `Command`, `Bookmark`, `BalloonShow`,
  `BalloonHide`, `Idle`, `Move`, `DragStart`, `DragComplete`, `Click`, `DblClick`,
  `VisibleState`, `ActivateInputState`, `ListeningState`, `ActiveClientChange`,
  `DefaultCharacterChange`, `AgentPropertyChange` [D].

## 2. Character file format (.ACS local / .ACF+.ACA streamed)

.ACS is the single-file local format; .ACF/.ACA split animation/audio so they can
be fetched individually over HTTP (`Get`/`Prepare` prefetch) [D]. Structure [B]:

- **Header**: magic `0xABCDABC3`, locators for CHARACTERINFO / ANIM / IMAGE / AUDIO.
- **CHARACTERINFO**: version (all four official chars: 2.1), GUID, frame size
  (Merlin/Genie/Robby 128×128, Peedy 160×128), flags (`0x110220` on all four =
  standard-animation-set + voice + balloon), **VOICEINFO** (TTS engine GUID
  `CA141FD0-AC7F-11D1-97A3-0060082730FF` family, per-character voice mode GUID,
  speed **155–157 words/min**, pitch 50–130, langid, gender, age, style),
  **BALLOONINFO** defaults (2 lines × 28–32 chars/line, fg `#000000`, bg
  `#FFFFE1`, border `#000000`, MS Sans Serif −13/400), palette, **tray icon**
  (present on all four), **STATEINFO** (state → animation-name list), localized
  name/description/reminder strings for ~30 locales.
- **ANIM record**: name (stored UPPERCASE), `transition` byte, explicit return
  animation string, frames. Observed transition values: `1` = return via exit
  branches, `2` = no transition; the explicit return string is **empty for every
  animation in all four official characters** — returns are done via exit
  branches, and "named returns" (e.g. `GetAttentionReturn`) are ordinary
  animations the *application* is expected to play [B+D].
- **Frame**: one or more (image, x, y) layers; `audio` ref (0xFFFF = none) —
  sound effects are **bound to frames** [B]; duration in centiseconds;
  `exit_to` frame index (exit-branch target, −1 = none); branch list
  `(frame, probability)` for random/loop branching; **mouth overlays** (image,
  type, replace, x, y, w, h, optional region) — the per-frame lip-sync overlays
  that make an animation "speakable" [B].
- Inventory [B]: Merlin 73 anims / 614 images / 32 audio; Genie 76/591/14;
  Peedy 85/977/30; Robby 68/593/32.

## 3. State model

The server has exactly **16 states**, verified as UTF-16 strings in
`AGENTSVR.EXE` and as STATEINFO keys in every official .ACS [B]:

```
Showing  Hiding  Speaking  Listening  Hearing
IdlingLevel1  IdlingLevel2  IdlingLevel3
MovingUp  MovingDown  MovingLeft  MovingRight
GesturingUp  GesturingDown  GesturingLeft  GesturingRight
```

> The contract's `Greeting/Thinking/Searching/Executing/Blinking` are **not**
> Agent-server states — see §9 (they belong to the Office Assistant layer).

Merlin's binary state table (identical pattern in Genie/Peedy/Robby) [B]:

| state | animations (random pick) |
|---|---|
| Showing | Show |
| Hiding | Hide |
| Speaking | RestPose |
| Listening | Alert |
| Hearing | Hearing_1..4 |
| IdlingLevel1 | Blink, Idle1_1..4 |
| IdlingLevel2 | Idle2_1, Idle2_2, Blink, Idle1_1..4 |
| IdlingLevel3 | Blink, Idle3_1, Idle3_2 |
| Moving{L,R,U,D} | Move{Left,Right,Up,Down} |
| Gesturing{L,R,U,D} | Gesture{Left,Right,Up,Down} |

State trigger semantics [D]:

- **Showing/Hiding** — played when user *or* app shows/hides; also set the
  window's Visible flag. Show animations end at neutral; Hide animations begin
  at neutral; a blank first/last frame gives a clean cut [D].
- **Speaking** — *fallback only*: used when a `Speak` follows an animation that
  has no mouth overlays. Canonical content: single RestPose frame with mouth
  overlays (matches binary: Speaking → RestPose, RestPose has overlays) [B+D].
- **Listening** — plays when the user presses the push-to-talk hot key (or
  `Listen(True)`); should be short/attentive (Merlin: Alert).
- **Hearing** — plays when the *beginning of spoken input is detected*; the
  server **auto-StopAlls the queue and plays a Hearing animation** at utterance
  start [D]. Programmatic `Listen(True)` deliberately does *not* stop the queue
  or play the Listening animation — the app decides via the `ListeningState`
  event [D]. Listening mode has a fixed, **undocumented timeout** [U]; re-calling
  `Listen(True)` resets it; `ListeningState` completion causes:
  ProgramDisabled / ProgramTimedOut / UserTimedOut / (user finished) [D].
- **Moving/Gesturing** — see §5/§6.
- Multiple animations per state; the server picks **uniformly at random** [D+B].
  Looping animations must not be assigned to states (they'd need a Stop) [D].
  A state with no assignment is skipped — only required if the app triggers it [D].

### 3.1 Idle selection and interruption

- After an **idle timeout** (duration undocumented [U]; the server just knows
  "user input stopped"), the animation services begin playing **IdlingLevel1**
  animations. The idle levels are **progressive**: level 1 for the first idle
  period, level 2 for the second, then level 3, which persists **until canceled
  by any new animation request** (any Play/Speak/MoveTo/user interaction) [D].
- Each idle period plays one random animation from the level's list, then the
  server re-arms its idle timer [D]; design guidance: L1 = blink/glance,
  L2 = reading, L3 = sleeping; many subtle variants, never attention-grabbing [D].
- `GetIdleOn/SetIdleOn` per character lets an app **turn automatic idling off**
  and drive idling itself; the `Idle(dwCharID, bStart)` event reports idle
  processing start/stop [B strings + D].
- Interruption rule: any new request cancels the idle cycle and resets to level 1
  on the next idle period [D].

**vs Bonzi** (BONZI432_RE §2): Bonzi's engine is a VB `Timer` tick
(`tmrCharacterAction_Timer`, ~1 s [INFERENCE there]) with a countdown re-arm and
`RandomActionEvent` picker that only fires while the *user* is idle — same
observable shape as MS Agent's server-internal idle service, but Bonzi mixes
idle animations with content gags (jokes/facts/time), which MS Agent leaves to
the application layer.

## 4. Animation queue, Play/Stop/Interrupt, Return animations

- Per-client FIFO queue: requests play **sequentially** ("after the previous
  request completes") [D]. `Play(name)` queues an animation; playing an
  invisible character plays it invisibly and still completes the request [D].
- **Return/exit transitions** [D+B]:
  - If the *previous* animation has a Return defined **via exit branches**, the
    server automatically plays the exit branch (jump to the frame that returns
    to neutral) before starting the next animation. Exit branches must be
    acyclic and terminate at the last frame [D].
  - If the Return is an **explicit separate animation** (e.g. `ReadReturn`), the
    *application* must `Play` it explicitly — Agent will not [D].
  - Office's character guidance: every animation except Goodbye/Greeting/Show/
    Hide should start and end at RestPose, use exit branching, and define **no**
    explicit returns, because "Microsoft Office does not play explicit Return
    animations" [D].
- `Stop(reqID)` stops one request and removes it from the queue; the VB control
  `Stop` stops the current animation [D]. `StopAll(typeBitfield)` flushes
  request classes (PLAY/GET/MOVE/SPEAK/HOLD/VISIBLE; only `VISIBLE=0x20`
  captured verbatim, rest partial [D-partial]).
- `Interrupt(reqID)` **halts another character's current animation** (e.g. break
  a loop) and starts the next queued one — it does *not* flush the queue, and a
  character cannot interrupt itself (the call would queue behind itself) [D].
- `Wait(reqID)` holds this character's queue until *another* character's request
  completes — cross-character choreography [D].
- **Looping animations** (frames branching back forever — Merlin's `Processing`,
  `Searching`, `Thinking`, `Reading`, `Idle3_2` etc. [B branch data]) block the
  queue until `Stop`/`Interrupt` [D].
- FAQ edge case: if the Speaking state animation is not defined, Return after
  MoveDown/Left/Right/Up misbehaves — always define a simple Speaking animation [D].

## 5. Movement, edges, taskbar

This is the biggest model difference from Bonzi.

- **MS Agent has no edge-walking, no gravity, no floor.** A character is a free
  2-D sprite window; `MoveTo(x, y, speed)` plays the appropriate **Moving state
  pose animation** (server picks Up/Down/Left/Right from the delta, direction is
  *character-relative*), then **slides the window** to the target over `speed`
  ms (recommended 1000; `speed=0` teleports with no animation). The last frame
  of the moving animation is what you see while the frame slides — "there is no
  support for animating the character while its frame moves" [D]. Contrast
  Bonzi (BONZI432_RE §1): animated legs, walking along the bottom of the work
  area, walk-home on load, `clsBBuddyMoveClass` state machine.
- Position is the **top-left of the animation frame**, screen coordinates [D].
  `SetPosition` moves instantly without animation [D].
- **On-screen keeping** [D+B]: `Move` event and `GetMoveCause` include
  `SystemMoved = 4` — "the server moved the character to keep it onscreen after
  a screen resolution change" [D]. Binary evidence that the server tracks the
  work area and taskbar: imports of `SystemParametersInfoA` (SPI_GETWORKAREA),
  `GetSystemMetrics`, `EnumDisplayMonitors`, `MonitorFromPoint/Rect/Window`, and
  a registered **`TaskbarCreated`** window message [B]. Exact clamp policy
  (full containment vs partial) is undocumented [U]; [I] the server clamps the
  sprite to the monitor work area and re-positions on taskbar move/resize and
  resolution change.
- **User drag**: dragging the sprite is built into the server — `DragStart` /
  `DragComplete` events (button/keystate + pointer), then `Move` with cause
  `UserMoved` [D]. The user can drop the character anywhere on screen [D].
- **Visibility causes** [D]: `GetVisibilityCause` ∈ NeverShown, UserHid,
  UserShowed, ProgramHid, ProgramShowed, OtherProgramHid, OtherProgramShowed,
  UserHidViaCharacterMenu, UserHidViaTaskbarIcon; `VisibleState` event carries
  the same enum. This is how a well-behaved client knows *who* hid the shared
  character (e.g. don't auto-re-show after the user hid you).
- **Taskbar/tray**: a character compiled with a tray icon (all four official
  ones have one [B]) gets a notification-area icon with a pop-up menu (Show/
  Hide, Voice Commands, properties); `AutoPopupMenu` toggles the character's own
  right-click menu [D]. The **Voice Commands Window** is positioned adjacent to
  the speaker/tray icon, remembers its last position across sessions, lists
  host-app commands first then global ones, and is resizable/scrollable [D].

## 6. GestureAt

`GestureAt(x, y)` — server computes direction from character position to target
and plays the matching **Gesturing state** animation; gestures are
character-relative [D]. If the gesture animation has mouth overlays, the
character can speak while gesturing [D].

## 7. Speech, sound, and word-sync timing

- `Speak(text, url)` [D]:
  - TTS path: pass `text`; `|`-separated alternatives are randomly picked per
    processing ("variety without re-calling") [D].
  - Audio path: pass `url` (.WAV, or .LWV linguistically-enhanced which carries
    its own text); `text` then supplies balloon text only [D].
  - **Speaking-animation selection**: the server re-uses the **last animation
    played** — if that animation has mouth overlays, the character speaks *over
    it*; otherwise it falls back to the **Speaking state** animation [B+D].
  - If the audio channel is busy, **the text still shows in the balloon** (only
    if balloon `Enabled`) — audio never blocks the visual channel [D].
  - Balloon word-breaking on whitespace; CJK needs U+200B markers; set
    `LanguageID` before speaking [D].
- **Lip-sync timing** [D]: automatic. TTS engines drive mouth overlays from
  visemes; plain .WAV gets generic flapping; .LWV (built with the MS
  Linguistic Information Sound Editing Tool) carries word/break marks for
  accurate sync. Per-character TTS defaults are compiled into the file:
  speed 155–157 wpm, pitch 50–130 [B]. `GetTTSSpeed` returns wpm (read-only;
  tags can vary per utterance) [D]. Volume range 0–65535; FAQ warns to use 1
  instead of 0 to mute (0 crashes AgentSvr on NT/2000/XP) [D].
- **Speech markup tags** [D]: `\chr \ctx \emp \lst \map \mrk \pau \pit \rst
  \spd \vol` — backslash-delimited, case-insensitive, whitespace-sensitive,
  reset after each Speak; engine-dependent subset. Notables:
  - `\map="spoken"="balloon"` — say one thing, display another [D].
  - `\mrk N\` — bookmark; fires the `Bookmark(BookmarkID)` event when the
    spoken position is reached → the app's hook for **word-position-synchronous
    actions** (mid-speech animation swaps) [D].
  - Only `\mrk` and `\map` apply to sound-file output [D].
- `Think(text)` — **thought balloon** (distinct puffy style), supports only
  `\mrk`, and **does not change animation state** [D].
- **Frame sound effects**: animations carry per-frame audio refs (Merlin: 32
  audio clips bound across 73 animations) [B]; gated by per-character
  `SoundEffectsOn` *and* the user's global sound-effects setting [D].
- **Balloon pacing** [D]: `AutoPace` style bit — balloon reveals text **word by
  word at the current output rate** (i.e. paced to the voice), scrolling when
  text overflows; off = whole text at once. `AutoHide` — balloon hides after a
  timeout; `SizeToText` — balloon shrinks to content. Defaults come from the
  character file (2 lines × 28–32 chars, `#FFFFE1` bg, MS Sans Serif −13 [B]);
  per-client overrides via `IAgentBalloon(Ex)` [D]. `BalloonShow`/`BalloonHide`
  events go to all clients of the character [D].

## 8. Voice commands and input

- Global listening hot key is user-configured (property sheet); `HotKey`
  property is read-only [D]. Commands are registered per client
  (`Commands.AddCommand(name, caption, context, sound)`), shown in the Voice
  Commands Window and the character pop-up menu [D].
- `Command` event: fires when the input-active client's command is recognized or
  menu-picked; carries Name + Confidence (−100..100) + Voice text + two
  alternates; menu picks return Confidence 100 with empty Voice text;
  `Count=0` means heard-but-unmatched; server-global commands (Hide,
  ShowAllCharacters, …) return empty Name but still deliver the heard text [D].

## 9. The Office Assistant layer (where Greeting/Searching/Executing/Blinking live)

The Office Assistant (Clippy et al.) is an *application* on top of MS Agent with
its own state machine — the contract's `Greeting / Idle / Thinking / Executing /
Searching / Blinking` are Office states, not Agent-server states [B (§1) + D].
Office 2000 characters use the **Office animation set** [D]:

- `Greeting` — elaborate appear (blank → RestPose) when the assistant is
  **chosen**; `Goodbye` — elaborate disappear when another is chosen.
- `Idle1_*` / `Idle2` / `Idle3` — mapped to "no user input" with escalating
  durations (same progressive-idle model as §3.1).
- Looping task animations with quick entry/exit: `Thinking`, `Searching`,
  `Processing`, `Writing`, `Save`, `Print`, `SendMail`, `CheckingSomething`,
  `GetWizardy`, `GetArtsy`, `GetTechy` — played while the corresponding Office
  operation runs (≈ Office's Think/Execute/Searching states).
- 8-direction `Look*` set, `Alert`, `Wave`, `Explain`, `Congratulate`,
  `GetAttention`, `EmptyTrash`, `IdleHit`.
- Office plays **no explicit Return animations** and drives the character mostly
  in a fixed corner (no MoveTo choreography) [D].

Takeaway for bonzibuddy-rs: a "notify" state machine (greet → think → execute →
announce → blink-idle) is an *application-layer* pattern over the same 16-state
server model — exactly how Bonzi's scheduler sits above its own engine
(BONZI432_RE §2).

## 10. Maps to bonzibuddy-rs (gaps + file-level suggestions)

Bonzi's engine already mirrors MS Agent in several ways (frame-carried audio
fired once per frame entry = §7 [B]; `return_anim` metadata; loop-until + Stop
semantics; user-idle gate ≈ §3.1 trigger). Concrete gaps, MS-Agent rule → file:

1. **Progressive idle levels** — MS Agent: L1 → L2 → L3, reset on any activity
   (§3.1). bonzibuddy-rs `pet.rs` uses one flat `IDLE_POOL` with a uniform
   5–10 s re-arm. *Suggestion*: split pools by name prefix (`IDLE1_*`,
   `IDLE2_*`, `IDLE3_*`, `BLINK` in L1+L2 like Merlin [B]); keep a level counter
   advanced per idle fire, reset in `set_user_idle(true→false)` transition and
   on any `push()`. Keep the 5–10 s cadence (that matches *Bonzi*'s scheduler,
   BONZI432_RE §2; MS Agent's own timeouts are [U]).
2. **Speak-over-last-animation selection** — MS Agent speaks over the last
   played animation if it has mouth overlays, else the Speaking-state fallback
   (RestPose) (§7). `pet.rs::Tell` picks a random `TALK_POOL` animation instead.
   *Suggestion*: in `Tell(anim, text)`, if `anim` has mouth overlays (assets.rs
   already parses `mouth` per frame) speak over it; for bare `Speak(text)` use
   `RESTPOSE` + overlays as the Speaking fallback rather than keeping whatever
   animation was running.
3. **Return-before-next on interruption** — MS Agent auto-plays exit-branch
   returns when a running animation is replaced (§4). `pet.rs::set_anim` cuts
   immediately; `return_of`/`assets.return_anim` are only consulted on natural
   completion (`anim_finished`). *Suggestion*: when `push()` replaces a
   non-looping animation that has a return, chain the return before the queued
   action (or fast-forward to the animation's `exit_to` frame — assets could
   carry the exit frame from the manifest the way `acs_parse.py` does).
4. **Speech-bubble timing** — MS Agent: AutoPace reveals words at TTS rate,
   AutoHide after a timeout, 2-line/28–32-char defaults, `#FFFFE1` bg (§7).
   `pet.rs::bubble_now` shows all text for a fixed `secs` (chat_reply estimates
   0.09 s/char); `render.rs` draws a static wrapped box. *Suggestion*: add
   `bubble_words: usize` advanced from the same wpm constant (≈156 wpm default
   [B]) while `speaking`, render only the revealed prefix (scroll overflow lines
   like AutoPace), and derive hide-delay from word count instead of char count.
   Adopt the `#FFFFE1`/black/2-line defaults in `render.rs` for authenticity.
5. **Word-sync / mouth timing** — MS Agent drives mouths from TTS visemes or
   .LWV word marks; `\mrk` bookmarks fire at spoken position (§7). `tts.rs` has
   no timing feedback and `pet.rs` flaps the mouth on a fixed 110 ms cycle.
   *Suggestion*: without viseme access (espeak-ng), (a) set flap period from
   wpm (~2 syllables/word ⇒ flap ≈ 60000/wpm/2 ms ≈ 77 ms at 156 wpm) instead
   of 110 ms constant, and (b) use the word-reveal clock from (4) as a
   pseudo-`\mrk` bus so staged animations can swap mid-sentence at word
   boundaries. Keep the existing "voice interrupts voice, music survives"
   `Effect` model — it already matches MS Agent's channel-busy behavior (§7).
6. **Spoken≠shown text** — MS Agent `\map="spoken"="balloon"` (§7).
   `agent.rs::parse_stage` already strips stage directions; extend the reply
   protocol with an optional split so TTS can get a cleaned variant while the
   balloon shows the full text (`clean_tts` currently feeds both identically).
7. **Work-area / taskbar awareness** — MS Agent keeps the sprite inside the
   monitor **work area**, reacts to resolution change (`SystemMoved`) and
   taskbar moves (`TaskbarCreated` message registration) (§5). `main.rs` walks
   and clamps against **full monitor bounds** (explicit code comment at the
   `reset_zoom` clamp: "winit has no work-area API") and bounces off monitor
   edges. *Suggestion*: on X11 read `_NET_WORKAREA` off the root window (and
   watch it with a PropertyNotify — the TaskbarCreated equivalent) to get the
   true floor (taskbar top) for the walk line and the clamp; on Windows use
   `SPI_GETWORKAREA`; on reconfigure, reposition proportionally instead of
   letting the pet sit under the taskbar. Keep Bonzi's bottom-edge walk model
   (BONZI432_RE §1) — MS Agent's slide-only movement is the *reference model*,
   not the target; the work-area part is what transfers.
8. **Move/visibility cause tracking** — MS Agent's cause enums (§5) let clients
   distinguish user-hide from program-hide. `pet.rs` has `hidden`/`hide_until`
   but no cause. *Suggestion*: add `last_hide_cause: User|Program` so tray
   "Show" vs user dismissal can be told apart (currently `hide_now()` always
   auto-reveals after 8 s regardless of who hid him).
9. **Notify semantics** — MS Agent's answer to "get the user's attention" is
   `GetAttention` (+ `GetAttentionContinued` loop until acknowledged) and the
   `Announce`/`Alert` animations, plus tray icon (§2, §9). bonzibuddy-rs has
   `GETATTENTION` in `FUN_POOL` only. *Suggestion*: give notifications a
   dedicated path: `AnimateLoop("GETATTENTION", …)` until click/ack, mirroring
   the Office Executing/notify pattern (§9).
10. **Idle on/off + Idle event** — MS Agent exposes `IdleOn` and the `Idle`
    event (§3.1). bonzibuddy-rs's `user_idle` gate is the same idea; consider a
    settings toggle ("let Bonzi idle on his own") wired to `next_idle`
    suppression, matching `SetIdleOn`.

## 11. Open / unresolved [U]

- Actual idle-timeout durations per level (server-internal, never documented).
- Exact work-area clamp policy (containment vs partial visibility at edges).
- 1.x `Request(Number)` message/button table (help file `AGT0409.HLP` is in the
  corpus but WinHelp LZFu decompression not attempted; TLB members verified [B]).
- `StopAll` bit values other than `VISIBLE=0x20`.
- MarvinDocs.chm / AGT0409.HLP full text (CHM/HEAP extraction not done; the
  lwef mirror covers the same ground).
