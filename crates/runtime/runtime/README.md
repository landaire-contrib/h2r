# Runtime ABI v2

Generated C11 includes `include/h2abi.h` and returns explicit transfers to the
Rust runner. Registers and memory addresses keep their guest widths. Ext80
registers and XMM registers are bytes, so the ABI does not depend on native
floating point or vector layouts.

| Type | Bytes | Alignment | Offsets |
| --- | ---: | ---: | --- |
| CpuAbi | 288 | 4 | GPRs 8, EIP 40, FS 44, EFLAGS 48, validity 52, x87 56, XMM 156, MXCSR 284 |
| X87Abi | 100 | 4 | registers 0, control 80, status 82, tag 84, opcode 86, FIP 88, FDP 92, selectors 96/98 |
| TransferAbi | 28 | 4 | version 0, size 4, kind 8, route 12, target 16, continuation 20, event 24 |

Generated objects exported for Rust registration use
`H2_DEFINE_GENERATED_ABI_NAMED(name)` with a unique symbol per object. The
zero-argument `H2_DEFINE_GENERATED_ABI()` form is translation-unit-local, so
multiple objects can use it safely inside C. Pass the named probe and handler
to `Dispatch::register_generated_object`. Runtime version queries use the
separate `RuntimeAbiVersion` identity; the Rust convenience query cannot coerce
to a generated C probe. The native `h2abi_v1_version` scalar ABI is unchanged.
Registration compares the object's compiled-in version with the runtime's.
The older explicit-version registration remains available. Duplicate generated
entries and resumes fail. Registering an override returns the displaced
`Override`, which can be restored with `restore_override`.

`Runner::run` enters a function. `run_from` accepts an explicit transfer after a
stop: use a Return for a guest return, or an explicit Resume for a generated
continuation. The exact resume map bypasses entry overrides; an entry transfer
applies them. Dispatch requires executable guest memory except for recognized
kernel tokens. Only Return to `HARNESS_EXIT` terminates the harness. Generated
code must spill the current EIP before an observable operation; otherwise a
handler's diagnostic PC is its entry PC. Native handlers must not retain CPU
or context pointers or unwind through a C frame.

Rust overrides save callback state in an owned continuation closure. With a
memory guard already held, `ctx.call_guest(cpu, &mut memory, target, closure)`
pushes a distinct guest return token and yields an ordinary Call. On Return to
that token the runner consumes the closure. Its captured state survives kernel
stops and later `run_from` calls. It does not use host recursion or process-exit
tokens. There are 1024 slots, with FIFO reuse and a generation in every token.
The control-only range is `0xb0000000..0xf0000000`; tokens use a 10-bit slot and
20-bit generation and need no PC alignment. A stale token cannot run or cancel
a newer closure in the same slot. Generations never wrap: slots retire at their
last generation, and exhaustion reports ContinuationLimit. The finite namespace
has over a billion distinct identities; this fails closed instead of allowing
old tokens to alias new state. Use `cancel_continuation(token)` after an
unwind or `clear_continuations()` during thread termination. The token is the
Call transfer's `resume_pc`. These APIs drop saved Rust state but do not repair
the guest stack. Slot reuse does not make abandoned token copies valid again. JUMP to
a live token cancels it and faults, while only RET invokes it. Release the
memory guard before handing the transfer to the runner.

`ExecCtx::runtime()` exposes shared services; `ExecCtx::memory()` borrows the
context for the lifetime of its memory guard. Reads, writes, `call` and `ret`
are Guest methods on that guard. These helpers acquire no additional locks.
Do not hold any Runtime memory guard while entering a runner or callback.
Ordinary generated accesses use checked helpers, or the optional gated C RAM
path below. Both synchronize with HLE memory guards. Atomic RMW,
XCHG, CMPXCHG and CMPXCHG8B hold it for the entire operation, including flags
and results. Atomic outputs may alias CpuAbi fields: the CPU borrow ends before
the final output store, which wins if it targets a field modified by the helper.
Output must remain disjoint from ExecCtx. Scalar widths are 1/2/4 bytes; CMPXCHG8B uses eight. XADD uses ADD
and the returned old operand. Bit operations use the decoded effective word
address. Ordinary accesses allow 1/2/4/8/10/16 bytes and explicit alignment.
Failure leaves memory and output unchanged and records an access fault.

Memory has 64 MB of physical backing. Sorted mappings preserve aliases and
holes; checked accesses use validated mapping-index hints with binary-search
fallback. Cross-mapping writes
validate the entire span before changing bytes. NV2A addresses report an
unsupported-device fault. The loader maps page-rounded headers and sections,
combines shared-page permissions, and honors each section's head/tail read-only
flags. The union can make an RX section share an RWX page with writable data or
headers. This is intentional for Halo 2's packed sections; each widened page
is recorded in `LoadedImage::permission_notes` with its effective permissions
and contributing regions' requests. Unused mapped page bytes start zero. TLS is read from loaded memory;
its index is validated after protections are final. `tls_data_size` records
aligned payload bytes plus the leading pointer slot. Guest startup still
performs its own TLS stores.

### Optional inline RAM accesses

Generated code uses `h2fastmem.h` unless built with `H2_FASTMEM=0`. The runtime
provides `h2abi_v1_fast_acquire`, `h2abi_v1_fast_release` and `h2abi_v1_exit`
(the release with the accounting charge, at every handler exit); a dynamic
bridge must forward all three. `ExecCtx::enable_fast_memory(false)` selects checked Rust
helpers in the same generated image for comparison or diagnosis. ABI v2's
architectural layouts and context preempt-word prefix do not change.

The fast path uses an eight-MB virtual-page table, allocated lazily per Guest.
It checks width, alignment, page extent, permission and physical bounds in C.
Cross-page accesses, missing/protected pages, gate contention and atomics use
the existing Rust helpers. A shared atomic gate excludes C from a Rust
`MemoryGuard`, preserving compound HLE operations and host-thread access.
The gate is inactive until a module requests a view. Runtime and ExecCtx
memory methods return `ffi::MemoryGuard`, which dereferences to Guest like
the previous MutexGuard. Explicit MutexGuard type annotations must change.

Mapping insertion, both protection APIs, unmapping and whole-Guest replacement
refresh or invalidate the view under the gate. Fast stores mark physical
pages; the next Rust memory guard publishes those bits to the existing
subscribers before exposing Guest. Thus aliases, watch changes and dirty
queries see the same page-level barrier semantics. The runner can use the
same gated table for execute checks, falling back for faults or contention.

The invocation view is owned by a raw-pointer allocation, created once with
`Box::into_raw` and reclaimed only when the context drops. Generated C keeps
that same pointer across slow accessor callbacks. Rust updates counts and
budgets through raw field pointers derived from it, never by reborrowing an
owning `Box<FastAccess>`. The regression
`ffi::memory::tests::retained_fast_view_survives_slow_callbacks` is Rust-only
and can run under Miri; it crosses a page boundary and executes an atomic
between simulated inline accesses.

Successful inline accesses accumulate in the context view. C sets the slice
wake bit on the exact access that exhausts the budget; slow helpers and every
handler exit settle the count into the context. No lock spans a handler,
helper call or transfer. A view or RAM pointer must not escape its invocation,
and accessor buffers must not alias the RAM view. See the emitter contract
for the full foreign-code protocol and `docs/perf/guest-memory.md` for measurements.

### Range protection, unmapping and physical reservations

These Guest methods serve MmSetAddressProtect (182), NtFreeVirtualMemory
decommit/release (199), MmFreeContiguousMemory (171) and Halo 2's fixed
physical arena. Signatures are in `API-CHANGES.md`. Every range method takes a
page-aligned `va` and non-zero page-multiple `len` (`va + len` may equal 2^32).
It validates the whole range before changing anything, so on error the mapping
list, permissions and bytes are exactly as before.

- `protect_range(va, len, perms)` sets permissions on exactly that range and
  splits mappings at its edges. Split pieces keep their physical offsets.
  Protection belongs to a virtual mapping: other aliases of the same physical
  bytes keep their own permissions and still share the bytes.
  `Permissions::NONE` (PAGE_NOACCESS) makes every access fault with
  Protection. The existing whole-mapping `protect(va, perms)` is unchanged.
- `unmap(va, len)` removes exactly that range, splitting at the edges. Bytes
  are never cleared: another alias still sees them. A physical page becomes free
  only when no mapping or reservation references it. `map_zeroed` uses
  never-touched RAM first and then reuses freed pages, zeroing them first.
  `map` with an explicit physical address does not clear anything; that is how
  aliases work.
- Errors: `InvalidRange` for bad bounds; `Unmapped` if any byte is unmapped
  (decommit only committed runs); `Claimed` if the range touches an HLE claim.
- Lifted code keeps Execute. Generated C checks Execute only when the runner
  dispatches into a handler, and then jumps locally across pages without
  rechecking. A page made non-executable or unmapped under running generated
  code would therefore keep running. The runtime does not pretend otherwise:
  `guard_code(va, len)` marks pages (rounded out) as lifted code, and the loader
  guards every executable XBE section. From then on `protect_range` without
  Execute and `unmap` over any guarded page fail with `LiftedCode` and change
  nothing. This applies to the `_owned` variants too. Write-protecting code
  (RX) is allowed. Whoever registers a generated module for code outside the
  image must guard that code too. The legacy whole-mapping `protect` is not
  guarded, because loader and kernel patching use it.
  Refusing is better than recording an `Unmodelled` event here. Guest has no
  event sink: events belong to an ExecCtx, and these calls run under the memory
  guard. A refusal also keeps the all-or-nothing contract. The kernel turns
  `LiftedCode` into a named stop, and it doesn't silently succeed.
- `physical_aliases(physical, len)` lists every mapped window onto those
  physical bytes, clipped to the query, so a kernel can propagate or audit
  changes across aliases.
- `claim_virtual(va, len, owner)` marks an HLE-owned range such as
  `HLE_RESERVED` `0xa0000000..0xa1000000`. Guest-driven `unmap` and
  `protect_range` then refuse any overlap. The owner calls `unmap_owned` or
  `protect_range_owned` with its `ClaimId` for ranges inside its claim. Claims
  do not block `map`/`map_zeroed`, so existing HLE allocators work unchanged.
- `reserve_physical(physical, len, aliases, owner)` withholds RAM from
  `map_zeroed`, `find_free_physical`/`physical_free` and explicit `map`. For each
  alias base `a`, the window `[a, a + len)` can be mapped only by
  `map_reserved(id, va, len, perms)`, at the same offset, so every alias sees the
  same bytes. Reserve on an empty guest: an existing mapping of those physical
  pages or in a window is `Overlap`. Reserved bytes start zero and survive
  unmap/remap. Reservations are permanent.
- `find_free_physical(len, range, alignment)` and `physical_free(physical, len)`
  answer occupancy with reservations included, rounding mappings out to whole
  pages. Kernel allocators must use them instead of scanning `mappings()`.

`load_xbe_with(bytes, &LoadOptions::halo2())` reserves `HALO2_PHYSICAL_ARENA`
`0x61000..0x3145000` (0x30e4000 bytes) with the window
`0x80061000..0x83145000`. It does this before headers, sections, the stack or
DATA exports get backing. `load_xbe` reserves nothing. From the retail XBE,
`0x0012b400` stores cursors 0x61000/0x3145000 at `0x004e6424..` and calls
`XPhysicalAlloc(0x30e4000, 0x61000, 0, PAGE_READWRITE)` at `0x0012b43e`
(`0x002d156a`, which becomes MmAllocateContiguousMemoryEx with low 0x61000 and
high 0x3144fff). It drops the result. Carvers such as `0x0001db10` subtract
from the cursor, OR in `0x80000000` and call XPhysicalProtect with 0x404. Budget
on the retail image: 0xf1c000 bytes lie outside the arena, the loader uses
0x5c7000 of them (image, stack, DATA), and 0x955000 (about 9.3 MiB) is left for
the kernel, HLE and the game's other physical allocations.

Kernel stop handlers preserve the ordinal, name, CPU, return word and fixed
stack argument words. DATA thunks point at shared guest storage; opaque object
storage and key cells remain unimplemented zeroed placeholders. Stack top
`0x70000000`, DATA base `0x80000000` and dispatch-token ranges are harness
choices. Full TLS/KPCR setup, callbacks listed in an XBE directory and kernel
service behavior remain outside this loader milestone.

A context retains at most 1024 recent events, evicting older diagnostics without
stopping execution. `take_events()` returns owned `(id, event)` records after a
stop; drain before eviction when complete logging is required. IDs remain valid
until drained or evicted, are not reset on drain, and wrap past zero and live
IDs. The first failing FFI access is pinned through eviction and drains until
its invocation propagates it, even if native output is malformed. Generated code
must stop at that first failure; subsequent helper faults from a broken handler
are ordinary, evictable diagnostics. HarnessExit is recorded normally and may
later be evicted or drained; it is not permanently pinned. EventLimit remains
as an API variant but retention no longer emits it. An unrecorded Fault gets an
explicit event.
The runtime denies unsafe code except in `ffi.rs`. The C probe is a development
dependency, so ordinary runtime builds do not compile or export test handlers.

Tests cover layouts and C struct returns on both Macs, page sharing, aliases,
atomics and flags, callback continuations, diagnostics and import metadata. No
nxdk export-definition file with `@N` decorations was available offline during
this review. Stack-word counts are checked mechanically against the documented
signatures; that is not an independent decoration check. The ignored local-disc
test reads `disc/default.xbe` directly without copying it:

```sh
cargo test -p runtime --test loader -- --ignored
```

## Write watch

`watch.rs` tells a consumer (D3D's texture cache first) which guest RAM
pages changed since it last looked. It is a dirty-page bitmap set inside
`Guest::write_bytes`, drained by the consumer at its own entry points.

```rust
impl Guest {
    pub fn subscribe_writes(&mut self, owner: &'static str) -> WatchId;
    pub fn unsubscribe_writes(&mut self, id: WatchId) -> bool;
    pub fn watch_writes(&mut self, id: WatchId, addr: u32, len: u32) -> Result<(), WatchError>;
    pub fn unwatch_writes(&mut self, id: WatchId, addr: u32, len: u32) -> Result<(), WatchError>;
    pub fn has_dirty(&self, id: WatchId) -> bool;
    pub fn take_dirty(&mut self, id: WatchId) -> Result<Vec<DirtyRange>, WatchError>;
    pub fn take_dirty_into(&mut self, id: WatchId, out: &mut Vec<DirtyRange>) -> Result<(), WatchError>;
    pub fn writes_unreported<R>(&mut self, id: WatchId, f: impl FnOnce(&mut Guest) -> R)
        -> Result<R, WatchError>;
    pub fn fold_address(&self, va: u32) -> Option<u32>;
    pub fn write_watch_owners(&self) -> Vec<(WatchId, &'static str)>;
}
pub struct DirtyRange { pub addr: u32, pub len: u32 } // folded, page-aligned
pub const watch::CONTIGUOUS_BASE: u32 = 0x8000_0000;
```

Addresses are in the folded view `0x80000000 | pa`, the one D3D reads
resource data at (`AddressMap::XboxContiguous`). `fold_address(va)` turns a
mapped virtual address into it. Ranges are rounded out to 4 KiB pages and
must lie inside `[0x80000000, 0x84000000)`, otherwise `InvalidRange`.

Guarantees:

1. **Complete.** Every change to RAM bytes made after `watch_writes`
   returns marks the page for each subscriber watching it. The RAM store is
   private to `guest.rs` and has three mutation sites, all reported:
   `write_bytes` (so every typed setter, `Guest::call`, the atomics,
   `h2abi_v1_write` and the atomic imports, the loader, and every kernel or
   HLE copy, since they all write through a `Guest`), the zeroing of a
   reused gap in `map_zeroed`, and the zeroing in `reserve_physical`. The
   optional generated-C RAM view also records physical dirty pages under
   the memory gate; the next Rust guard publishes them before exposing
   Guest. A future interpreter binding or DMA engine must use a reported
   path too.
2. **Aliases folded.** Bits are per physical page, so a store through any
   virtual alias marks the same page and is reported once, as
   `0x80000000 | pa`.
3. **Conservative.** A page is reported if any byte of it was stored, even
   with the value it already had. A faulting store changes nothing and
   reports nothing (`write_bytes` is all-or-nothing). Map, unmap and
   protect calls change no bytes and report nothing.
4. **Ordered.** The bit is set before `write_bytes` returns, under the
   memory lock. A consumer that drains while holding the guard has seen
   every store that completed before it took the lock, from any thread.
5. **Drain.** `take_dirty` returns ascending, disjoint, non-adjacent runs
   and clears them. Subscribers are independent. `unwatch_writes` drops
   pending bits of the pages it unwatches.
6. **Own stores.** `writes_unreported(id, f)` mutes `id` for exactly the
   stores made inside `f`. Since `f` holds the only guard, those are the
   consumer's own stores. Other subscribers still see them. The mute is
   lifted even if `f` panics.

Why a bitmap and not host page protection: ordinary generated stores use
`h2abi_v1_write` or the gated inline RAM view (RUNTIME-CONTRACT.md 1.1). Both
paths record physical pages at the store, without a host protection fault. `mprotect` plus a SIGBUS handler would need RAM in its own
`mmap`, a signal (microseconds) on the first store to each watched page
every frame, an `mprotect` call per drained run, and a signal handler that
runs while the store holds the memory mutex. It would also miss nothing the
bitmap misses, because there is only one store path.

Cost, measured with `cargo test -p runtime --release --test write_watch --
--ignored --nocapture` (M1 Max, 4-byte stores spread over 64
mappings, best of 5):

| case | ns per store |
|---|---|
| master, no watch code | 16.8 |
| no subscriber | 16.5 |
| subscriber, store to an unwatched page | 18.7 |
| subscriber, store to a watched page | 22.7 |

An override entry that drains and runs its body muted costs about 18 ns
when nothing is dirty and 67 ns with one dirty page (including that store).
`watch_writes`/`unwatch_writes` rebuild the 2 KiB union bitmap, about 256
word ORs per subscriber.

## Guest interrupts

`interrupt.rs` runs a guest function asynchronously, the way a vblank
interrupt would. Any thread can queue one on the shared `Runtime`:

```rust
impl Runtime {
    pub fn raise_interrupt(&self, interrupt: GuestInterrupt) -> Raise; // Queued | Coalesced | Full
    pub fn pending_interrupts(&self) -> usize;
    pub fn clear_interrupts(&self) -> usize;
}
pub struct GuestInterrupt {
    pub target: u32, pub args: Vec<u32>, pub convention: CallConvention, // Cdecl | Stdcall
    pub thread: Option<u32>, pub coalesce: bool, pub name: &'static str,
}
impl ExecCtx {
    pub fn set_interrupts_masked(&mut self, masked: bool);
    pub fn interrupts_masked(&self) -> bool;
    pub fn in_interrupt(&self) -> bool;
}
```

Delivery:

- **Where.** At the top of the runner loop, once a handler (generated code,
  an override or a continuation) has returned a valid CALL, JUMP or RETURN
  and before the next is dispatched. CPU state is canonical there. Not at
  a RETURN to `HARNESS_EXIT`, not while the context is masked, and not
  while a delivered callback has yet to return (no nesting). An empty queue
  costs one atomic load per transfer.
- **Which.** The oldest queued interrupt whose `thread` is None or the
  context's `thread_id`. The queue holds at most `MAX_PENDING` (64);
  `coalesce` drops a raise while one with the same target and thread is
  queued.
- **Frame.** The whole `CpuAbi` is snapshotted. The arguments are pushed
  right to left below ESP, then a callback token as the return address
  (`call_guest`). DF is cleared. The target is entered as an ordinary CALL,
  so it shows in backtraces.
- **Return.** When the target returns to the token, ESP must equal the
  value its convention requires (Cdecl: just above the token; Stdcall:
  the ESP at the safe point). The snapshot is then restored whole, GPRs,
  EFLAGS, x87, XMM, MXCSR and EIP, and the interrupted transfer continues
  unchanged. Memory changes stay.
- **Failures.** A wrong ESP faults with `Event::InterruptImbalance`. A push
  that faults is a `MemoryFault`, and the continuation limit is
  `ContinuationLimit`. Each restores the snapshot and drops the interrupt.
  A callback that stops (a kernel call) is resumed with `run_from` as usual
  and stays in service until it returns. One that unwinds past its token
  (`cancel_continuation`, `clear_continuations`) ends the service without a
  restore.
- **Latency.** A safe point comes at every call, return and cross-function
  jump, and, through the preempt word, at the next loop header: a raise
  sets `preempt::INTERRUPT` after queueing, so a guest spinning on a vblank
  counter inside one generated function leaves its loop within one trip
  and the interrupt is delivered (see "Preemption"). Rust override code
  that loops without returning to the runner still has to deliver it
  itself.

## Preemption

`preempt.rs` lets the host take the CPU from a guest loop that calls
nothing. Every `ExecCtx` owns a preempt word and starts with a pointer to
it (`#[repr(C)]`, offset asserted; the reason for ABI v2), which generated
code reads once per handler and polls at each loop header. The `Runtime`
keeps weak references to the words so a request from any host thread can
reach them all. While the word is set a
loop leaves with `JUMP`/`RESUME` to its header (registered as a resume),
which reaches the runner's safe point like any other transfer.

```rust
pub const SLICE: u32 = 1;     // the armed context spent its accesses
pub const INTERRUPT: u32 = 2; // an interrupt is queued
pub const REQUEST: u32 = 4;   // the host wants the running thread to yield
impl Runtime {
    pub fn request_preempt(&self, reasons: u32); // every context, any host thread
    pub fn preempt_pending(&self) -> u32;         // OR over every context
}
impl ExecCtx {
    pub fn arm_preemption(&mut self, accesses: u64);
    pub fn disarm_preemption(&mut self);
    pub fn preemption_armed(&self) -> bool;
    pub fn preempt_pending(&self) -> u32; // this context's word
    pub fn slice_spent(&self) -> u64;     // accesses since armed (or taken)
    pub fn take_slice_spent(&mut self) -> u64; // and reset to 0
    pub fn take_preempted(&mut self) -> Option<TransferAbi>;
}
```

- **Safe point.** At the top of the runner loop, once a valid CALL, JUMP or
  RETURN is in hand and before it is dispatched. With the word clear this
  is one relaxed load. Otherwise the runner takes the context's bits (one
  swap), then delivers a pending interrupt if it can. If it delivered one
  it puts back every bit but INTERRUPT and dispatches the interrupt call,
  so the next safe point decides the slice. Taking before delivering means
  a raise that lands in between leaves its bit set rather than lost.
- **Armed.** If the context is armed and its slice ran out or REQUEST was set, the
  runner keeps the undispatched transfer (`take_preempted`) and returns
  `STOP` with event 0. `run_from` with that transfer continues exactly
  where the thread was. `kernel::Machine` arms each thread slice.
- **Unarmed** (the default, a DPC, tests): the bits are dropped and the run
  carries on. INTERRUPT alone never takes the thread away.
- **Spent.** `slice_spent` counts every access since the context was armed
  or the count was last taken, past the budget too. `kernel` takes it at
  each kernel call and after each run and charges it to logical time.
- **Slice.** Per context: the armed context that runs out sets SLICE on its
  own word, and only it acts on it. Counted in calls to the generated-code accessors (`h2abi_v1_read`,
  `_write` and the atomics) by an armed context: reproducible, and a
  spin-wait always reads memory. Rust overrides do not count. A loop that
  touches no memory at all stops only for INTERRUPT or REQUEST; this is
  not instruction-count or wall-clock preemption, and a busy register-only
  loop can keep the CPU until one of those arrives.
- **Limits.** A REP string instruction (`Stmt::Repeat`) runs its own inner
  C loop with no poll: it is finite, but a long one yields only at the next
  loop header or transfer after it. Rust override loops are not polled.
- **Interrupts.** `raise_interrupt` sets INTERRUPT on every word after
  queueing, and again on a coalesced raise; unmasking with interrupts
  queued sets it on the context's own word. So a spinning generated loop
  returns to the runner within one trip of an interrupt becoming
  deliverable. INTERRUPT alone never takes the thread away.

## Generated modules

The emitter's contract is `crates/recompiler/emit/RUNTIME-CONTRACT.md`. The runtime
exports the two shims declared in its `h2gen.h`:

- `h2shim_v1_event(ctx, pc, saved_eip, code, arg)` records codes 1-7, 13 and
  14 as `Event::GuestException` and codes 8-12 as `Event::GeneratedStop`, and
  returns the event id. Exceptions, and codes it does not know
  (`Event::UnknownShimEvent`), are latched like a failed memory access, so the
  invocation ends in FAULT even if the handler ignores the id.
- `h2shim_v1_unsupported(ctx, pc, mnemonic, detail)` copies both strings into
  `Event::Unsupported` and returns its id. Generated code then stops.

Both events carry a guest backtrace. The runner keeps a best-effort call chain
per context: it records each CALL it routes and drops records once ESP rises
above their return slot. `ExecCtx::backtrace(pc)` reads it, and
`ExecCtx::report(id)` renders any retained event with its PC, instruction and
frames. The chain is diagnostic only and never affects where a RET goes.

`Dispatch::register_generated_module(&h2gen_module, h2gen_module_abi_version)`
checks the module probe and table version, then each entry row's own object
probe, kinds, duplicates, terminators and that every resume owner is a kind-0
row of the module. It registers nothing unless the whole module is valid, and
errors name the table and row. Entry rows of kind 0 and 1 dispatch with
`start = addr`; resume rows dispatch their owner with their `start`. Returns
check the resume table before entry dispatch.

## Performance instrumentation

See [performance](../../../docs/perf/guest-memory.md) for opt-in per-context counters,
coarse timers, generated `H2_PERF=1` counters, and the menu FPS acceptance test.
