# Steam Machine boot: measurements

Evidence for [../steamos.md](../steamos.md). Newest first.

## 2026-11-20: first boot to the main menu

Commit: `fd962f92b` on the `linux-buck` branch (PR #5), working copy with
the deploy fixes that followed it.

Machine: Valve Steam Machine (DMI product "Fremont"), AMD Navi 33 GPU,
SteamOS 3.9.5 (build 20261023.100), Game Mode. The game ran as a Steam
shortcut in the Steam Linux Runtime 4 container.

Build: `buck2 run -c build_mode.mode=release //platform/steamos:run` with
`H2_OPT` unset (the module at `-O1`, clang tiers off).

Method: `h2 agent` on the device over SSH, against the running game:

```sh
cd ~/devkit-game/h2r
./h2 agent status
./h2 agent frame snap 2
./h2 agent faults
./h2 agent fallbacks
./h2 agent capture 300
./h2 agent hostmetrics
```

Result:

- `status`: map `mainmenu`, phase `menu`, about 30 fps.
- `frame snap 2`: pass 2 drew 646 draws. The snapshot showed the menu scene (not kept: it is game output).
- `faults`: 0 ended, 0 generated. `fallbacks`: 0 sites, 0 interpreted.
- `capture 300` at the menu: busy p50 29.6 ms, p95 35.0 ms, max 51.0 ms against the 33.3 ms budget. 156 of the 300 frames went over on `guest_cpu`.
- `hostmetrics`: 25.5 ms of game CPU per tick, 104% process CPU.

The music stalled now and then, in step with the late ticks. That led to
building the device module at `H2_OPT=2` by default.
