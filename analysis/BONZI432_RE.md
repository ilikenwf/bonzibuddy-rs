# BonziBuddy432.exe payload — deeper RE (walk semantics, behavior scheduler, telemetry call sites)

Static analysis only (radare2 6.x + Ghidra 11 headless). Binary was never executed.
This report extends `README.md` / `bonzi_exe_urls.txt` (string-level catalog) with
**call-site-level** findings: addresses are virtual addresses in the analyzed PE image
(image base 0x400000). Uncertain claims are tagged `[INFERENCE]`.

Analyzed primary target: `analysis/extracted2/6` — the 2004 main engine (3.4 MB,
PE32 VB6 native, dated 2004-02-09). Secondary: `extracted2/5` (1999 engine),
`extracted2/7` (engine variant with Storybook content).

---

## 0. Payload mapping (BonziBuddy432.exe → cab → files)

`BonziBuddy432.exe` is a Delphi installer stub (~100 KB code, `SizeOfCode=0x18c00`)
with an embedded cab (`payload.cab` / `data_fixed.cab`, 12 entries). Cab entry → file map
(sizes cross-checked against the installer manifest strings in `installer.strings.txt`):

| cab entry | file | role |
|---|---|---|
| 0 | 344 KB DLL (emsmp.dll / ActiveSkin) `[INFERENCE]` | installer skin |
| 2 | 140 KB DLL — `BonziTapFilters.dll` `[INFERENCE]` | IE tap filter |
| 3 | 1 MB `BonziBDY.EXE` (path `C:\Source\VB\BonziBuddy\Distribution\Free Version\BonziBDY.EXE`) | 2004 **free** engine — **no Winsock import** (network stripped) |
| 5 | 815 KB, 1999-11-24 | original 1999 engine (`BonziBuddy E-Mail Reader`, Winsock import) |
| 6 | 3.4 MB, 2004-02-09 | **main 2004 engine** (`im.bonziworld.com`, privacy policy, `BonziTapFilters`, Winsock import) — primary target of this report |
| 7 | 2.5 MB | same engine family (identical form set) + Storybook content — `BonziBDY_35.EXE`/`_4.EXE` variant `[INFERENCE]` |
| 8 | 49 KB | `BonziBuddy_Killer.exe` (uninstaller) |
| 9 | 155 KB | `BonziCHECKERS.ocx` |
| 10 | 28 KB | `BonziCTB.dll` |
| 11 | 5.5 MB | game bundle (Solitaire / Jigsaw / Beach Checkers) |

All engines are **VB6 native code**. Entry 6 imports **only `MSVBVM60.DLL`** (270 imports,
all runtime helpers) — the engine has **zero direct user32/gdi32/kernel32 imports**.
Every OS interaction (windows, screen metrics, timers, files, sockets) goes through the
VM runtime or registered ActiveX controls.

VB6 object table at `0x4384b0` (80 entries, 0x30 bytes each: `{f18, name, codeStart, codeEnd, ...}`)
enumerates all forms/modules/classes: `frmAgent` (210 methods), `clsBBuddyMoveClass`,
`clsBonziEventQueueManager`, `clsBonziEvent{Skeleton,Item,Product,Update,Daily,Email,Tap}`,
`frmBBIM*` (BonziWorld IM), `frmEmailQueue`, `frmDownloader`, `modTaps`, `modRandom`,
`modTimeAPI`, `modGOLDMembers`, etc.

---

## 1. (a) Walk / movement semantics

**The engine never moves a window itself.** There are no `SetWindowPos`/`MoveWindow`
call sites (no user32 import at all). The character sprite window is owned and animated
by the **MS Agent server** (`AgentObjects.Agent.1` control, `Agent1` = frmAgent field
`+0x22`); the engine only issues high-level `Characters("Bonzi")` commands.

### Character object and confirmed vtable slots

Character object global: `DAT_0073a08c`. Confirmed early-bound vtable slots
(from Ghidra decompile of `FUN_00664e00` and others):

| slot | method | evidence |
|---|---|---|
| `+0x64` (25) | `Play(animation)` | `(**(code **)(*DAT_0073a08c + 0x64))(DAT_0073a08c, L"Explain", …)`, `L"Blink"`, `L"Alert"` |
| `+0x78` (30) | `Speak(text)` | calls with TTS markup strings, e.g. `L"\spd=160\Local and World News! \spd=200\Weather! …"` |
| `+0x748` | trace/debug log (`DisplayDebugData`) | called with two strings (event name + detail), gated by debug flags `DAT_0073a21c` / `DAT_0073a0ae` |

`Agent1` control itself: slot `+0x8` = `Characters.Load` (see LoadBonzSub below).

### LoadBonzSub — character load + initial walk home

`FUN_006b99e0` @ `0x6b99e0` (trace target `0x6ba39b`):
- `(*frmAgent[0x22])[8]` → `Agent1.Characters.Load("Bonzi")` (character name string
  `DAT_0045a900` = `"Bonzi"`, fetched via late-bound `__vbaLateIdCallLd` default-property
  pattern).
- Trace strings `"frmAgent.LoadBonzSub loadbonz 1"` and `"move to home position"`
  (`0x6ba39b`) — after load, the character is sent to its home position.
- Uses `DAT_0073a254` — the global **`clsBBuddyMoveClass`** instance (created via
  `__vbaNew2` at `0x641185`, class vtable `0x431838`), slot `0x6e` called here.

### clsBBuddyMoveClass — the movement state machine

12 methods (from the class dispid table in the object name region):

```
SetCharacterPositionToHomePosition   RequestMoveToHome
RequestMoveToForm                    CompleteMoveRequest
CancelAllMoveRequests                SetHomePosition
```

Pattern `[INFERENCE]`: `RequestMoveToHome/Form` → `Character.MoveTo(x,y)` →
MS Agent animates the walk and fires `RequestStateChange`/move-complete events →
`CompleteMoveRequest` callback. The engine never polls position during a walk.

Candidate direct vtable call sites `call [reg+0x60]` (slot adjacent to `Play`):
`0x67bb82`, `0x699ba1`, `0x6be418`, `0x724e14` — each with a single argument, so these
are more likely `Wait(0)`/`StopAll` than `MoveTo(x,y)`; the exact `MoveTo` slot was not
resolved `[INFERENCE]`.

```asm
; 0x67bb78 — one-arg call on the character object (slot 0x60)
0x0067bb78  mov eax, dword [ebp - 0x3c]
0x0067bb7d  mov ecx, dword [eax]
0x0067bb7f  push 0
0x0067bb81  push eax
0x0067bb82  call dword [ecx + 0x60]
```

### Home position, work area, walk speed

- **Home position is persisted in the registry**: `GetSetting("BONZIBUDDY","UserInfo","SnapHome")`
  (rtcGetSetting thunk `Ordinal_689`, section ptr `PTR_DAT_0043b010`); `SetHomePosition` /
  `SetCharacterPositionToHomePosition` maintain it. Note: the function `FUN_00664e00`
  ("SnapHome" behavior) is *not* a window snap — it is the scripted behavior where Bonzi
  speaks `"From my home, we have access to all sorts of great things…"` (Speak slot `+0x78`).
- **Screen/work-area handling is delegated to MS Agent.** The engine has no
  `SystemParametersInfo`/`SPI_GETWORKAREA` call sites (no user32 import); screen size comes
  from VB's `Screen` object, implemented inside `MSVBVM60` `[INFERENCE: VM-internal]`.
  Taskbar avoidance, screen-edge clamping and the walk animation/speed are all
  **MS Agent server-side behavior** — the original engine has no walk speed constant of
  its own. The clone's 2000 px/s walk is a reimplementation choice, not a reversed value.
- Idle walk-in-place animations are played by name: numbered idles `Idle1_24`,
  `Idle2_1/2/25`, `Idle3_1/2/3` (string refs in the `0x466c70` region and `0x693920`
  idle-selection function `FUN_0x6b63b0`) via `Play`.

---

## 2. (b) Behavior scheduler

### Core tick: `tmrCharacterAction_Timer` @ `0x6aa640`

A VB6 `Timer` control (`tmrCharacterAction`, name in frmAgent control-name table at
`0x40f2c`). Design-time `Interval` is stored in the form stream and was **not extracted**
`[INFERENCE: ~1 s tick]`. All cadence is expressed in **timer ticks**, not ms.

Per-tick flow (Ghidra decompile + r2 disasm; VB6 pushes args so Ghidra lost some operands):

1. Refresh date stamp; trace via slot `0x748` (`"trmCharacterAction_Timer"` + counter).
2. Decrement countdown `DAT_0073a1d6` (short). If `> 0`: do nothing this tick.
3. At zero: **user-idle check** `FUN_006a91b0` — uses `rtcGetTimer` (`0x6a945b`/`0x6a946b`)
   to measure time since last mouse/keyboard activity.
4. If idle → trigger a random action via frmAgent vtable slot `0x287`.
5. **Relax mode**: registry-gated by `GetSetting("BONZIBUDDY", …)` keys
   `RelaxWhenIdle` (default `"Relax"`, key str `0x451a04`), `RelaxIdleTime` (`0x4515f4`),
   `NeverRelax`, `StayInRelaxOnLoad`; idle tick counter (word `[obj+0xe1]`) saturates at
   30000 ticks. `RelaxModeSwitch` (dispids 106/111/140/163/166, code region `0x6b7d90`)
   toggles the reduced-activity mode.
6. Slot `0x27` check (IsBusy/IsSinging-style guard) `[INFERENCE]`.

### Countdown re-arm (what sets "how long until the next action")

After each action the countdown `DAT_0073a1d6` is re-set to a small tick count —
discovered write sites (pattern `C7 05 d6 73 07 00 <n>`):
`0x650a97=10`, `0x66d6f3=5`, `0x6738da=0`, `0x673b1e=10`, `0x68255b=6`, `0x683b3d=6`,
`0x683fd0=6`, `0x6844f6=6`, `0x68471d=6`, `0x685df7=6`, `0x686697=6`, `0x68bae0=6`,
`0x68c170=6`, `0x68c800=6`, `0x68cc90=6`, `0x695d22=5`.
So: **an action fires roughly every 5–10 timer ticks**, and `SetNextRandomAction`
(dispids 16/120/160/163) recomputes it with `Rnd` after each action.

### RandomActionEvent @ `0x6953f0` — the action picker

Trace strings `"Entering Random Action Sub"` (`0x466394`) and `"RandomActionEvent"`
(`0x463c94`). Two-level random selection using VB `Rnd` (`rtcRandomNext`):

```
level 1: Select Case Int(8*Rnd + 1)   ; const 8.0 @0x40a8fc, 1.0 @0x402440
         rtcRandomNext @0x6955be, jump table @0x696033 (8 cases)
  cases 1,2,6,7,8 → call [obj+0x960] with arg 2   ; random idle playback [INFERENCE]
  case 3 → "Search" behavior: Play "Search" (str 0x442914) + Speak
  case 4 → call [obj+0x7ec] with -1
  case 5 → "Explain": Play "Explain" + Speak, then
level 2:   Select Case Int(5*Rnd + 1)             ; const 5.0 @0x404de0
                                               ; jump table @0x696053 (5 cases)
           mostly → call [obj+0x960] arg 2
```

5 of 8 top-level cases (plus most of the second tier) land on the same random-idle slot
`0x960` → the pet is heavily weighted toward silent idle animations; talking/searching
behaviors are the minority draws. A `Wait(100)`-style delay (`push 0x64` @ `0x69568c`)
follows the Search case. Slot `0x960` exact name unresolved (form vtable at `0x432b90`
has 209 public entries; the runtime vtable includes control accessors) `[INFERENCE]`.

### Content picker

`GetRandomAction` @ `0x6983e0` region: draws joke (`J001.nbd`), fact (`T001.nbd`),
product (`P001.nbd`) etc. from the local `.nbd` content files, then `Play` + `Speak`.
`SetRandomIdleState` @ `0x6b6c20` is a thin wrapper that traces its name via slot `0x748`
and dispatches the named idle action `[INFERENCE]`.

### Event queue (server-driven scheduled behaviors)

`clsBonziEventQueueManager` (`QueueManager` dispid 12/127/129/130/149/155,
`QueueMessage` 100/141/175, `RequestEnQueue` 116/118/119/138/144/170/173) with event
classes `clsBonziEvent{Skeleton,Item,Product,Update,Daily,Email,Tap}` carrying
`StartTime`/`FinishTime`/`ShouldBeHeard`/`MutuallyExclusive` and
`ParseRandomAction` (dispids 132/137/166). Timer management: `EnableTimers` @ `0x6950b0`,
`DisableTimers` @ `0x695250`, `Start/StopEventQueueTimeoutTimer` (dispids 169/170).
This is how downloaded promos (site/joke/quote of the day, product pitches) get injected
into the idle schedule — see §3.

### Tap / voice triggers

`modTaps` holds regex-like natural-language trigger patterns (e.g.
`"(Tell|me|...)[a|another] Joke…"` strings at `0x43e014`–`0x43f554`) that map user
click-menu/voice input to behaviors. `modTimeAPI` wraps `timeGetTime`; `modRandom`
wraps Rnd seeding.

**The 1999 engine (entry 5) uses the same architecture** (Timer tick + Rnd + MS Agent),
confirming the scheduler design is stable across versions.

---

## 3. (c) Telemetry / phone-home call sites (entry 6 unless noted)

Transport: **MSINET.OCX** `Inet` control (`Inet1_StateChanged` handler ref @ `0x67d754`),
a hidden **WebBrowser** instance (`m_MIE_DocumentComplete` / `m_QuietMIE_*` handlers), and
the **Winsock** control for the BonziWorld IM service. (Entry 3, the 2004 *free* engine,
has no Winsock import — network stripped.)

### Local spy logs (write sites)
| what | where |
|---|---|
| `Browse.nbd` browsing-history append | path built `0x61e562` (`DATADIR\` + `"Browse.nbd"` str `0x43bda4`), file write/close `0x61e5ad` (`rtcFreeFile`) |
| `Search.nbd` search-query log | same pattern, region `0x61e1ec` |

Both logs use the odd-byte+1 obfuscation (decoded samples in `../assets/content/nbd_decoded.json`).

### Content / ad fetches
| URL / resource | call site | function |
|---|---|---|
| `http://buddy.bonzi.com/bonzibuddy/daily.nbd?` | `0x69d8f7` | `FUN_0069d570` (daily content) |
| `products.nbd` | `0x69bd88` | `FUN_0069b720` (product pitches) |
| `Prod001.nbd` | `0x69b85b` | product content |
| `updates.nbd` | `0x69a9f0` | update manifest |
| update manifest parse (`siteoftheday=`, `jokeoftheday=`, `animation=`, `currentversion=`) | `FUN_0067bdc0` | speaks `"The site of the day is: …"`, `"The quote of the day is: …"`, `"The joke of the day is: …"` |
| `HTTP://download.bonzi.com/updates/bbyupdate.exe` | `0x67d2ea` | updater launch, gated by `LastUpdateCheckDateTime` (`0x6547d7`) |
| `Sites.nbd` (ad link list) | `0x665ae1` | `FUN_00665990` |
| `Apps.nbd` (download-manager offers) | `0x6a4094` | `FUN_006a3db0` |
| `s1.nbd` | `0x610ec7` | `FUN_00610dc0` |
| entry 5 (1999): `webserver1.bonzi.com/scripts/download/bbdownload.asp` `0x4676e7`, `daily.nbd` `0x4b1d91`, `products.nbd` `0x4aff25`, `updates.nbd` `0x4af4b0`, `bbyupdatewithreader.exe` `0x4b4510` | | |

### Click / opt-in tracking beacons
| URL | call site |
|---|---|
| `home.asp?nav=bbmenu` (every menu open) | `0x650ddb` |
| `home.asp?loc=` (menu item) | `0x68fbf6` |
| `trackyesno.asp?trackcode=` (yes/no prompts) | `0x60a713`, `0x674efe`, `0x675208` |
| `track.asp?trackcode=LYCOSOPTIN` | `0x639d72` |
| `bmtoolbar.asp` / `imtoolbar.asp` (toolbar installs) | `0x6243aa` / `0x6f710f` |

### Registration, IM, spam
| what | where |
|---|---|
| `secure.bonzi.com/bonzibuddy/bbregisteruser.asp` | `0x639af9` |
| `Reg.nbd` (registration payload) | `0x63672a` (`FUN_00636330`) |
| `broker.asp` (gold-membership) | `0x733b37` (`FUN_00733200`); `b17signup` `0x65c994` |
| **IM/BonziWorld presence beacon** `bwclientpost.asp` | `0x6dd770` (`FUN_006dcdf0`) — reads `UserInfo/UserRegistrationNum`, requires ≥21 chars, then POSTs presence data; `frmBBIM*` forms + `@im.bonziworld.com` |
| `tellfriends.asp` (referral spam) | `0x68fa92` |
| `bonzibuddyfriend.asp` (BonziMAIL friend-invite spam) | `0x60fe68` |
| email queue sent over HTTP | `frmEmailQueue.SendMessageHTTP` `0x60e8b4` (`"Mail send failed (HTTP)."`) |
| Lycos search hijack: `search.lycos.com/main/setup.asp?src=bonzi&query=` | `0x699dd5`; `my.lycos.com/setup.asp?src=bonzi` `0x634adb`; `www.lycos.com/srch/` `0x678adb` |

The installer-level persistence (Run key, Startup LNKs, `RegisterDeskMate`-style
registration) is already catalogued in `README.md`; nothing new found there.

---

## 4. What this adds over the earlier catalog

1. **Walk**: proved the original has *no* client-side window movement — walking is MS
   Agent server-side; the engine only calls `MoveTo` via `clsBBuddyMoveClass`
   request/complete pattern and persists home in `UserInfo/SnapHome`.
2. **Scheduler**: full tick model — countdown-in-ticks (5–10 re-arm), idle-gated firing,
   two-level `Int(N*Rnd+1)` action picker with exact constants/jump tables, relax-mode
   registry keys, and the server-driven event queue that injects daily/product promos.
3. **Telemetry**: every phone-home string from `bonzi_exe_urls.txt` now has a function
   address (tables above), including the `bwclientpost.asp` IM beacon precondition and the
   Lycos search-hijack call site.

## 5. Open / unresolved

- `tmrCharacterAction.Interval` design-time value (form-stream property not parsed) —
  tick duration in ms unknown; cadence documented in ticks.
- Exact `MoveTo` vtable slot on the character object (slot `0x60` candidates take one
  arg; likely `Wait`/`StopAll`).
- Slot `0x960` (random-idle play) name — needs full form vtable reconstruction.
- MS Agent server-side walk speed/clamping lives in `agentsvrs.exe` (not in this payload).
