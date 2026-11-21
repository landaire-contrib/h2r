# Host typing in the virtual keyboard

Status: Built. On a Steam Machine in Game Mode, X11 typing reached the profile name field, and Steam's keyboard opened after pad input and closed with the screen. Typing on Steam's keyboard itself is not yet confirmed.
Code: `crates/extensions/game/src/keyboard_glue.rs`, `crates/apps/app/src/keymap.rs`, `crates/apps/app/src/steam_keyboard.rs`. Tests: the same files.
Switch: `H2_STEAM_KEYBOARD=0` turns off Steam's on-screen keyboard.

When the game's virtual keyboard is focused, host typing edits it directly.
Printable BMP characters insert, Backspace deletes, Enter runs Done, and Escape
runs Cancel/Back. Arrows keep their existing pad bindings, so they navigate the
on-screen keyboard. Outside this screen all keyboard pad bindings behave as
before. Cmd shortcuts and the F3/F5/F6 host controls remain available.

Backquote (`` ` ``) opens and closes the debug menu (docs/runtime/debug-menu.md). It is bound to nothing in the default table. While the menu is open the arrows, Enter, Space, Escape and Backspace drive it, and other keys are swallowed.

Text comes from winit's logical committed text, not physical key names: Shift,
dead keys and keyboard layouts work. IME is enabled while a field is active;
only committed text is submitted, never composition/preedit. The OS owns its
composition UI. Xbox UI text is effectively BMP: supplementary characters
(such as most emoji) and controls are ignored. Available glyphs still depend on
the game's fonts. Private-use characters become U+25A1, matching the native
on-screen key handler's filter.

The window queues bounded owned edits through the existing KeyboardMouse
handle. A guest-thread hook after the normal UI frame update checks focus and
calls the original game functions. No window callback accesses guest memory.
A changed field, modal dialog or focus loss discards stale edits. Captured keys
remain suppressed until release, so holding Enter cannot select the next menu.

## Steam's on-screen keyboard

On Linux under Steam, such as a Steam Deck or Steam Machine in Game Mode, Steam's on-screen keyboard opens when the virtual keyboard screen gains focus and you last used a controller. It closes when the screen loses focus or the game exits. If you last used the keyboard or mouse, nothing opens.

Steam types into the game window as ordinary key presses, so its text takes the same committed-text path as a physical keyboard. Its Enter runs Done and its Backspace deletes. Close it with B to keep using the game's own on-screen keyboard.

The game does not use the Steamworks SDK. It writes `steam://open/keyboard` and `steam://close/keyboard` to the Steam client's command pipe, `~/.steam/steam.pipe`, which the Steam Linux Runtime container shares. The write never waits. It needs `SteamGameId` in the environment, which Steam sets for every game it launches.

## Retail trace

These are semantic fields and addresses from static retail analysis; no
executable bytes, layout tables or extracted strings are stored here.

Factory 0x0023784f and constructor 0x0023788f create screen ID 0xa7 with vtable
0x00459ba0. Opening helpers 0x00238c21/0x00238c69 select shared UI channel 2,
window 4. The channel is at 0x0054dba8: active screen +8, pending screen +0xc.
Global modal channel 0 and shared channel 1 have priority (dispatcher
0x00147dbe). Detection checks those, screen identity/controller mask, the
outgoing-transition flag, and a terminated writable buffer with a valid caret.
Clan creation disables host editing while the task at screen+0x372c is pending.

| Field | Meaning |
|---|---|
| screen+0x610 | entry context |
| screen+0x618 | original text saved for Cancel |
| screen+0x818 | external UTF-16 buffer pointer |
| screen+0x81c | caret pointer into that buffer |
| screen+0x820 | signed WORD capacity including terminator |
| screen+0x826 | keyboard page/shift flags |
| screen+0x828 | initial replace-selection flag |

Setup 0x002381de saves the original text, chooses the context limit, caps many
name contexts at 17 units, clamps the overall capacity to 150, positions the
caret at the end and sets initial selection. Many names thus allow 16
characters; a 16-unit clan buffer allows 15. The host never changes these limits.

| Entry | ABI and action |
|---|---|
| 0x00238932 | EAX = screen, one UTF-16 unit on stack, ret 4, AL = inserted; clears initial selection, shifts the suffix including terminator, checks capacity and advances caret |
| 0x00238a05 | EAX = screen, plain ret; native Backspace, including initial selection |
| 0x00238a63 | EAX = screen, direction on stack, ret 4; native caret movement (pad shortcuts keep using it) |
| 0x002382d6 | ECX = screen, AX = key index, plain ret; index 0x28 invokes Done |
| 0x00237f2a | ECX = screen, event pointer on stack, ret 4; type 5 / code 1 restores saved text then follows native Back/Cancel |

Native insertion accepts a UTF-16 unit; key selection filters private-use
characters through 0x00140420. Contexts gate accent/symbol pages. Done runs
0x00238114: per-context flags, character/name checks (including 0x00238ce9)
and optional callbacks, then the context-specific submit action. The host
calls this path rather than approving or closing a field itself. Invalid-name
dialogs, team creation and other native submission rules remain in control.

The installed frame hook is 0x001479c3 (one float argument, ret 4), called by
0x0012b450 each game frame. It runs the original first, then host edits through
runtime continuations, preserving the original function's CPU results and
stack position. The trampoline uses a mapped and claimed guest page. A missing
original skips installation with a diagnostic; existing overrides are refused.

Tests use synthetic screen memory and native-call fixtures. They cover focus,
bad pointers/limits/carets, modal/pending-task exclusions, action ABIs,
CPU/stack preservation, committed text/IME routing, bounded queues, stale edits,
held/repeated keys and unchanged arrow/pad routing. No game build or run.
