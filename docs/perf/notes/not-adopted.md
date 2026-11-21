# Performance: not adopted

Experiments that were tried and lost, so the same one is not built twice.
Each row: what it was, the result, and where the code went. The design
they were tried against is [../guest-memory.md](../guest-memory.md).

| Experiment | Result | Removed at |
|---|---|---|
| 4 KiB guest translation (`H2_FASTMEM=2` and a write-bit variant: a table mapping 4 KiB virtual to 4 KiB physical pages, with a held lease) | Covered 99.999% of menu accesses against 61% for the 16 KiB eligibility test, but used 22.4% more process CPU per frame than the inline path (medians of two 80-present menu runs each) and 15.3% more clang time; the write-bit variant did not differ | branch `fastmem-4k` at `894f2a3`, over master `c00124d`; never on master |
| f64 native x87 transcendentals (FSIN, FCOS, FSINCOS, FYL2X, F2XM1 through `libm`; switch `H2_X87_FAST_TRANSCENDENTALS`) | Tails of up to 2^41 units of the 64-bit result's last place for |x| < 10^6: not exact. Sweep in [guest-memory.md](guest-memory.md), "Handler entry and exit" | `2658859b9`, `361a99744` |
| A non-inlined leaf per access width for inline loads and stores | 1.5x faster than the helper path against 2.6x for the inline macro, with smaller code; dropped. [guest-memory.md](guest-memory.md), "Inline guest loads and stores" | not merged |
| A fixed-width switch in the per-access `h2_fast_copy` | No speedup established; not retained. [guest-memory.md](guest-memory.md), "Guest CPU: SSE, REP copies and D3D entry" | not merged |
| Write watch by Mach exception and page protection (`crates/recompiler/fastmem-proto`) | Prototype only; nothing in the runtime uses it. [fastmem-proto.md](fastmem-proto.md) | deleted after `0b3bbc32d` (architecture review, item 9) |
| `-march=x86-64-v3` (and `znver4`) for the Linux module through `H2_CPU` / `[h2] cpu` | Host 0.8-2.0% faster in a headless level; on the Steam Machine within noise at the main menu and in 03a. [steammachine-perf.md](steammachine-perf.md) | not merged |
