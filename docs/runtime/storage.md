# Xbox storage boundary

Status: Built.
Code: `crates/runtime/vfs/src`. Tests: `crates/runtime/vfs/tests/`, `crates/runtime/vfs/src/host_write_behind_tests.rs`.
Switch: `H2_MAP_DIR` (unset), listed in [switches.md](switches.md).

`crates/runtime/vfs` owns storage names, mounted filesystems, open file objects and
filesystem status results. It is safe Rust and has no dependency on guest
memory, `kernel`, `runtime`, or `cli`. The kernel translates guest pointers,
ANSI object strings, handles, information classes and IO_STATUS_BLOCKs at its
boundary. Nothing here extracts, copies or embeds the user's game files.

## Namespace and default drives

An Xbox file name is an object-manager name. `\??\D:` is a symbolic link,
whereas `\Device\CdRom0` names the DVD device. The hard disk exposes
`\Device\Harddisk0\PartitionN`. Partition1 contains TDATA/UDATA,
Partition2 is the dashboard partition, and Partition3–5 are utility/cache
partitions. These names and the T/U/Z roles are documented in the
[Cxbx storage implementation](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/support/EmuFile.cpp).

`Vfs::with_storage(Config)` mounts the device objects only and creates no
links: the kernel provides devices, while XAPI creates the drive letters itself
and treats an existing T: as a fatal collision. Neither the boot path nor
`with_disc` creates links.

Host tools without XAPI call `Vfs::create_standard_links(&Config)` afterwards,
which creates exactly D:, T:, U: and Z:. It does not roll back: if it fails
partway (for example `ObjectNameCollision` on a link that already exists), the
links created before the failure remain. The E:/C:/X:/Y: rows below are the
reference mapping and are not created by either call.

`Config::for_title` chooses `content::store::app_data_dir`:
`~/Library/Application Support/h2recomp` on Apple hosts, `%LOCALAPPDATA%\h2recomp`
on Windows and `$XDG_DATA_HOME/h2recomp` (`~/.local/share/h2recomp`) on
other Unix. `Config::new(root, title_id)` selects an explicit root. A missing or
empty variable is a configuration error (`InvalidParameter`), not a guest path
error. Halo 2's
title ID is `0x4D530064`.

| Link | Resolved device path | Host path relative to data root |
| --- | --- | --- |
| D: | `\Device\CdRom0` | User's XISO; mounted separately or by `with_disc` |
| E: | `\Device\Harddisk0\Partition1` | `disk/Partition1` |
| C: | `\Device\Harddisk0\Partition2` | `disk/Partition2` |
| T: | `\Device\Harddisk0\Partition1\TDATA\4D530064` | `disk/Partition1/TDATA/4D530064` |
| U: | `\Device\Harddisk0\Partition1\UDATA\4D530064` | `disk/Partition1/UDATA/4D530064` |
| X: | `\Device\Harddisk0\Partition3` | `disk/Partition3` |
| Y: | `\Device\Harddisk0\Partition4` | `disk/Partition4` |
| Z: | `\Device\Harddisk0\Partition5` by default | `disk/Partition5` |

The title suffix follows `Config.title_id`. Installed content (downloaded,
or installed from a disc such as the Map Pack, [map-pack.md](../game/map-pack.md))
lives under `T:\$c\<offer ID as 16 hex digits>\`, and so do new maps from
map packages ([maps.md](../game/maps.md) §4, offer IDs with bit 31 of the low half
set). `Config::title_data_dir`
and `Config::content_dir(offer)` give the host paths, and `vfs::offer_id`
builds an offer ID from a title ID and content number. X/Y and the default Z mapping
are this runtime's reference layout. A retail title's utility partition is
selected by XAPI; it is not permanently guaranteed to be Partition5. Select
3, 4 or 5 with `Config.utility_partition`, or delete/create the Z: link later.
No cache is automatically erased. Partition0 is the raw disk/config area,
not a FATX directory, and is intentionally unmounted. Additional partition
devices can be mounted explicitly; an unmounted device fails with
`ObjectPathNotFound`.

`Namespace::{create_symbolic_link, delete_symbolic_link, query_symbolic_link,
resolve}` implement the name portion of IoCreateSymbolicLink and related
exports. Links may target directories or other links. Longest matching
component prefix wins; `CdRom0` never matches `CdRom01`. Resolution is ASCII
case-insensitive, follows up to 32 links and bounds expansion length. A cycle
returns `ReparsePointNotResolved`. Link deletion/remapping affects subsequent
opens; an open file retains its original mount and path.

The API accepts both `D:\maps\file.map` and `\??\D:\maps\file.map`, as well
as full device paths. `D:`/`D:\` mean the drive root. There is no per-drive
current directory: `D:relative` is rejected. Backslash is the only separator;
forward slashes, NUL/control bytes, repeated interior separators, dot and
dot-dot components are rejected. A trailing separator requires a directory
in the absolute-name API. Relative opens take an existing directory handle through
`open_relative`, do not interpret drive letters, and reject traversal.

## Filesystem implementations

`Backend: Send` exposes a cheap `read_only` property and root-relative
`stat`, `read_directory`, `read_at`,
`volume_info`, `create`, `write_at`, `set_len`, `set_basic_info`, `remove`,
`flush` and final-object `cleanup`, plus owned initial-allocation hooks.
Mutation defaults return `MediaWriteProtected`. VFS owns share
checks, access checks, file positions, handle lifetime and enumeration state.
Backend methods independently validate their paths. A custom backend can
implement the same interface without any guest-memory integration. Backends
are trusted primitives: their write/resize/remove methods do not recheck a
file's READONLY attribute; permission decisions belong to the VFS open and
disposition paths. Host OS permissions still apply.

`Vfs::format_device(device)` quick-formats one mounted device by exact name
through `Backend::format` (default `MediaWriteProtected`). The format fails
with `SharingViolation` while any file object or pending delete exists on
that volume. `HostBackend::format` swaps the data and metadata roots for
staged empty siblings by rename. It rolls back completed renames if a
swap fails and clears caches and pending metadata after both swaps succeed.
Cleanup of the old trees is best effort. A tree left aside stays outside
the guest namespace for the next mount to sweep.

On Unix, the format code in `crates/runtime/vfs/src/host_format.rs` holds
descriptors for each root and its parent from mount time. It identifies the
mounted root by device and inode, finding it under its new name if it moves
within that parent. If the root leaves the held parent, the format fails.
Staging, renames, rollback, directory sync, and cleanup use the held
descriptors. A moved ancestor therefore leaves format anchored to the
original parent. Format syncs the parent directories before cleanup,
which opens directories without following symlinks and unlinks the links
themselves. Windows uses paths for swaps and rollback within the app's
private directories and deletes old trees without following links inside
them.

`Vfs::device_of(name)` gives the mounted device a name finally resolves
into. XAPI uses it to distinguish the [Z: and N: cache volumes](xapi.md).

`DiscBackend<R: Read + Seek + Send>` uses `xiso::Xiso` to discover the game
partition and index directory records. `DiscBackend::open(path)` uses a
read-only host file; `new(Cursor<Vec<u8>>)` works for synthetic tests. Both
plain XISO and Redump offsets follow `crates/formats/xiso`. Reads seek directly to
`partition_offset + sector * 2048 + requested_offset`, and allocate no
full-file buffer. The original disc is never modified. Writes, creation,
truncation, deletion and writable opens fail with `MediaWriteProtected`.
The existing XISO API exposes directory/file kind and size, not all raw
attributes or timestamps: the VFS synthesizes READONLY/DIRECTORY attributes,
returns zero times and serial, and estimates volume extent from the indexed
records. Those values are not a physical DVD geometry or volume ID. Unaddressable
record names, and descendants of an unaddressable directory, are skipped
without failing the whole mount. Structural XISO corruption still fails.

`RewriteBackend::new(inner, rewrite)` serves some of another backend's files
with different contents (`Vfs::with_disc_rewrite` wraps the disc in one). For
a path the `Rewrite` wants, the whole inner file is read once and handed to
it; the result is kept in memory and gives the file's size in `stat` and
listings and its bytes in `read_at`. Other paths, and files over 64 MiB, pass
through, with a `Rewrite::skipped` notification so dependent substitutions
can stop. A short source read is rejected instead of rewriting a truncated
file. It is read-only and never writes the inner backend. The runner uses
it for the font files ([button-icons.md](../input/button-icons.md)).

### Map overrides

`MapOverrideBackend::new(inner, dir)` serves `maps\<name>.map` from
`<dir>/<name>.map` when the inner backend (the disc) has that map and the
directory holds a file of that name. The boot mounts the disc through it
when `H2_MAP_DIR` names a directory (`crates/extensions/game/src/map_override.rs`);
unset or empty, the disc is mounted with `with_disc`/`with_disc_rewrite`
exactly as before. The font rewrite wraps it, so both apply.

- Only names exactly one level under `maps` ending in `.map` are
  overridden, and only ones the disc has: a new name stays missing,
  everything else falls through. Names match ASCII case-insensitively on
  both sides, like the disc.
- An override's size is reported by `stat` and in listings of `maps`
  (sector-rounded allocation, the disc's READONLY attributes, zero times).
  `read_at` streams from the host file (seek and read, no buffering, no
  size cap below the 4 GiB XDVDFS limit). `host_file` gives the override
  file, so telemetry and the speaker index read the edited map.
- The choice is made once per map, on first use, and logged once
  (`map override: maps\<name> from <path> (<n> bytes; disc <n> bytes)`).
  The open file is kept for the session, so a later replacement is seen
  only after a restart; a file truncated under it reads as `FileCorrupt`.
- Refused with a warning, falling back to the disc's map: a symlink whose
  target leaves the directory (a symlink inside it is followed), anything
  but a regular file, two host names that differ only in case, a file
  over 4 GiB, a file that cannot be opened. A missing or unusable
  directory is one warning and the disc throughout.
- Read-only: writes, creation, resizing and removal fail with
  `MediaWriteProtected`, and the directory is never written.

### Map packages

At boot, `boot::new_maps` scans `<data>/maps/` and registers a VFS
content path for each generated offer that fits in the multiplayer list.
The paths live under `T:\$c\<offer>\<name>.map`, with a placeholder
file on the data partition. A retail-based mod's generated name keeps
its path separate from the disc and Map Pack base paths.
[Generated offers](../game/maps.md#package-offers)

The VFS resolves the manifest at the mod path against its base from the
disc, installed Map Pack or a local whole-map layer. PPF and IPS reads
stream byte runs over the base. BPS reads use the materialised
`<data>/maps/cache/<MapId>.map` file. The resolved size is available
through `stat`, and a streamed overlay can be read through the VFS even
though it has no contiguous host file. A damaged layer or mismatched
base fails the mod read. [VFS content maps](../../crates/runtime/vfs/src/content_maps.rs) ·
[Patch resolution](../game/maps.md#2-applying-layers-at-load)

Live installs and removals publish a new registry and trigger a content
scan on the next main-menu tick. An open file keeps its resolved bytes
until the last handle closes. The normal boot mounts the disc, campaign
additions, any explicit `H2_MAP_DIR` override and the font rewrite
independently of the generated offers. [Live refresh](../game/map-select.md#changes-during-a-session) ·
[Boot mounts](../../crates/extensions/game/src/map_override.rs)

`Vfs::with_disc_backend(config, backend)` is `with_disc` for a disc
backend built by the caller. Tests: `crates/runtime/vfs/src/map_override.rs` and
`crates/runtime/vfs/tests/map_override.rs` (sparse 300 MiB streaming, listings,
case, fall-through, symlinks, a no-match mount identical to `with_disc`,
and a model of the cache copy with its checksum).

`HostBackend::new(root, metadata_root)` presents a host directory as FATX.
The metadata tree must be separate, with neither root inside the other.
Each parent directory has a cached index of ASCII-insensitive names,
so lookup and create collisions behave consistently on APFS, case-sensitive
APFS and Linux filesystems. Original spelling is preserved. Two preexisting
host entries that differ only by case report `FileCorrupt` rather than
arbitrarily choosing one. Host symlinks, sockets/FIFOs/special files, files
over the FATX size limit, and (on Unix) multiply linked files are hidden from
listings; direct access to such objects fails. Names outside the supported
FATX rules are also hidden. These neighbors do not poison ordinary directory
queries or creation. Entry limits count eligible names/kinds without reading
sidecar metadata.

Mac host litter (`.DS_Store`, `._*`, `.Spotlight-V100`, `.fseventsd`) is reserved
and hidden at every level, including entire litter directory subtrees. It
does not count toward item limits or logical allocation. It cannot be
created or addressed through guest paths. Deleting a guest-empty directory
first removes these recognized litter entries, including their directory
subtrees. Litter symlinks are unlinked without following them. Other invisible
objects are left untouched and can still prevent physical removal; that
failure preserves the directory's logical metadata.

Use one VFS instance under a kernel mutex for all guest threads and one
backend per physical directory. Ordinary guest file operations resolve host
paths for each request and rely on the mounted paths staying stable. Share
reservations apply within a single VFS instance.
Directory indexes refresh on a parent-directory modification timestamp change
and are updated incrementally by VFS create/write/resize/remove. Repeated lookups do not enumerate
the parent again. Logical allocation is scanned once and updated incrementally
on successful creates, writes, resizes and removals. External changes can
stale the usage cache or same-timestamp indexes: recreate the backend after
editing the data root externally. This ownership constraint also avoids
share/identity races with host replacements.

### FATX rules and logical metadata

`open_allocated` and `open_relative_allocated` carry NtCreateFile's initial
allocation hint. HostBackend reserves cluster-rounded logical capacity on
create/overwrite without extending EOF. VFS passes a unique file-object identity
to `Backend::create_allocated` / `overwrite_allocated`, then calls
`release_allocation` when that object's last handle closes. Independent opens
cannot trim another object's reservation; duplicates share the owner's lifetime.
Writes consume the reservation; EOF changes or its owner's release trim unused
capacity. Ordinary `cleanup` still handles flushing and timestamps separately.
An ordinary existing-file open ignores the hint. Capacity failures precede
creation/truncation. These are session
reservations, not physical host disk preallocation. See [checkpoint acceptance](checkpoints.md).

The FATX on-disk directory record has a 42-byte name and a 32-bit size; see
[XboxDev's FATX definitions](https://github.com/XboxDev/fatx/blob/master/libfatx/fatx.h)
and [raw record layout](https://github.com/XboxDev/fatx/blob/master/libfatx/fatx_internal.h).
VFS enforces 1–42 ASCII bytes per component, including the extension, and
`u32::MAX` bytes per file (4 GiB minus one byte). **The 42-byte limit does not
apply to the XDVDFS disc backend**; its directory records have an 8-bit name
length, and VFS reports a 255-byte maximum there.

The supported printable ASCII set excludes `" * + , / : ; < = > ? \ |`.
In particular, **`+ , ; =` remain invalid**. This follows the actual
`FatxIllegalCharTable` bitmap in [nboxkrnl's original-Xbox FatxIsNameValid](https://github.com/ergo720/nboxkrnl/blob/master/nboxkrnl/io/hdd/fatx.cpp#L164),
not its incomplete comment: word `0xFC009C04` sets the bits for these four
characters as well. A regression decodes that bitmap across all printable
ASCII characters. The [FATXplorer author](https://fatxplorer.eaton-works.com/fatx-file-system-limitations-reference)
also states that its validation follows Xbox/Xbox 360 rules. The looser
[OXOS validator](https://github.com/kooscode/oxos/blob/main/kernel/src/filesystems/hddfs/volume.cpp#L296)
disagrees; it is not used as evidence for broadening this character set.
The earlier Velocity-only citation has been replaced with original-Xbox
evidence. Control characters and non-ASCII bytes remain unsupported.
Names preserve trailing spaces and periods; they are not Windows-normalized.
`.` and `..` cannot be stored. Xbox object strings are byte strings; complete
OEM/codepage decoding and uppercasing of bytes above 0x7f is not yet modeled.
Rejecting them is an explicit compatibility limit, not a claim that Xbox
uses Unicode or that every high byte is illegal on hardware.

The emulation additionally limits a volume-relative path to 250 bytes
(excluding device prefix) and a directory to 4,096 visible items. Public
references disagree about full-path and per-directory limits across Xbox
versions/tools. These are explicit VFS policies based on the
[FATXplorer author's limitations reference](https://fatxplorer.eaton-works.com/fatx-file-system-limitations-reference),
not a claim that every original Xbox kernel enforces identical limits.
Limits include expanded TDATA/UDATA prefixes, so aliases cannot bypass them.

Metadata uses NT FILE_ATTRIBUTE bit values: READONLY `0x01`, HIDDEN `0x02`,
SYSTEM `0x04`, DIRECTORY `0x10`, ARCHIVE `0x20`; NORMAL `0x80` is accepted as
input and has no stored bit. Unsupported attributes fail. DIRECTORY is
structural and cannot be toggled via basic information. New files have
ARCHIVE; successful writes/truncation set it. READONLY blocks writable opens
of existing files and new delete requests. It does **not** revoke rights already granted to
an open handle: creating a new readonly file with WRITE and then writing,
or setting READONLY after opening a writer, both work. WRITE_ATTRIBUTES can
clear the flag. Readonly directory attributes do not gate child creation.
These are logical attributes, independent of host POSIX permission bits; a host permission
failure still propagates. The supported original-Xbox attribute set is also
listed by the [FATXplorer author](https://fatxplorer.eaton-works.com/fatx-file-system-limitations-reference).

`Timestamp(u64)` is NT time (100 ns since 1601 UTC). FATX's packed timestamp
format uses a 2000 epoch and two-second precision, as shown in the
[XboxDev FATX date/time definitions](https://github.com/XboxDev/fatx/blob/master/libfatx/fatx_internal.h).
VFS rounds down to two seconds and clamps to 2000–2127, using UTC so behavior
is deterministic across host timezones. Creation/access/write times persist;
change time is an additional emulated field. Reads update access time in
memory and coalesce it per path; the record is persisted on explicit flush,
file-object cleanup/close, a later metadata write, or backend drop. This
avoids a write+fsync per read. Write/truncation updates write/change times
and sets the archive bit. Those stay pending in memory too ("Write
metadata" below); `stat` sees them at once. Imported host files start
from host timestamps; subsequent VFS timestamps are logical, not a promise
to rewrite host birthtime or mtime.

### Write metadata

A write or truncation does not store the metadata record on the spot.
Storing it durably cost two fsyncs per guest write: while Halo 2 fills
its cache partition on the main menu, that was about a tenth of each
frame (docs/perf/menu-budget.md). A synthetic bench put it at 16 ms per write
against 0.09 ms (`cargo test -p vfs --release bench_write_metadata --
--ignored --nocapture`). The new write and change times and the archive
bit are stored:

- at `flush` (NtFlushBuffersFile), final cleanup (close), and backend
  drop, as access times are;
- by the backend's `vfs-metadata` thread, which checks every 250 ms for
  entries older than `vfs::METADATA_STALE` (1 s);
- by the write path, once the oldest pending entry is that old, but only
  if the thread could not be spawned. While the game streams writes, a
  store on the write path once a second (two syncs) was most of what
  `touch` cost the game thread;
- by `vfs::flush_pending_metadata()`, which reaches every live backend,
  leaked ones included. `h2 run` calls it when the run ends normally.

**Guarantee:** a crash, `process::exit`, an aborting panic, or a signal
(SIGINT, SIGTERM, a kill; nothing flushes on a signal)
loses at most about 1.25 s of write-time, change-time and archive-bit
updates. File data is covered by "Write-behind" below: at most the
bytes still queued (4 MiB) when the process dies without
`flush_pending_metadata`, which lands them first. A host power loss is
different: only `flush` syncs data, as before. A record lost that way falls back to the
previous record, as a torn one does. The bound relies on the flusher
thread; if it cannot be spawned (logged), only the write path, flush,
close and drop store pending metadata. Every record store holds the
pending map's lock, so a thread's store never lands after a newer one.
Removing a file holds that lock across the unlink and drops the pending
entry only once the file is gone, so a failed remove keeps both the
durable record and the write's pending metadata. Creating a name drops
any pending entry for it and holds the lock from clearing an orphaned
record until the new entry exists. Format holds the lock across the root
swap and clears pending metadata once the volume is empty; a failed format
moves nothing, so every pending entry still describes its file.

### Write-behind

All guest threads run on one host thread, so a host `write` stalls the
whole game, not just the guest thread that issued it. While Halo 2 copies
maps to its cache partition on the main menu (256 KiB NtReadFile and
NtWriteFile pairs), a 250 Hz profile put 10.8% of that thread in file
calls, most of it (355 of their samples) in the write syscall itself. `HostBackend` therefore queues the host data write
on a per-backend `vfs-write-behind` thread and returns.

What stays on the calling thread: path resolution,
capacity and growth checks (DiskFull, FileTooLarge), allocation and usage
accounting, the dirty set, write metadata (`touch`), and the returned
byte count (the whole buffer: the queued write is a `pwrite` loop that
writes all of it). Only the bytes travel, copied into a reused buffer
(a fresh 256 KiB allocation is a fresh mapping, and faulting it in cost
more than the copy), with an `Arc` of the kept-open handle; an evicted
handle's last copy closes on the writer thread. One FIFO thread writes them, so writes land in issue
order, per file and overall; overlapping writes resolve as they would
synchronously.

What the guest sees while bytes are in flight:

- `stat`, directory listings and the EOF/allocation every later call
  checks use a pending end of file per host path: the furthest byte any
  queued write to it reaches, combined with the host length (looked up
  first, so a write landing in between is still counted). Usage is the
  incrementally updated count, so volume queries agree too.
- Anything that reads or changes a file's host bytes or identity first
  waits for that file's queued writes: `read_at`, `set_len` (and
  `overwrite_allocated`), `flush`, `cleanup` (which flushes a written
  file), `remove`, `create` at the path, and `host_file` (an outside
  reader sees the bytes). `format` and backend drop wait for all of them,
  and so does `vfs::flush_pending_metadata`, so `h2 run`'s normal exit
  lands queued data before it stores metadata.
- `flush` keeps its meaning: queued data lands, then the file is synced.

Host write errors: the guest's write already returned success. The first
failure for a path is logged once and latched; the next operation on
that file that waits for it (`read_at`, `set_len`, `flush`, `cleanup`,
`remove`, `create`), or the next `write_at`, returns it, once, and does
nothing else. Bookkeeping assumed the write landed, so usage and the
directory indexes are rebuilt from the host at that point. A successful
format drops latched errors with the files; a failed one keeps them. A drop logs nothing more: the failure was
logged when it happened. Nothing below the backend sees the difference
between a failure reported late and one reported at once, except that
a failed write's bytes may be partly on the host.

Memory is bounded: at most 4 MiB (`MAX_IN_FLIGHT`) is queued or being
written; a write that would go over waits for the writer to catch up (one
write larger than the bound is admitted once the queue is empty). A
sustained write rate above the host's therefore throttles the caller
back to synchronous speed, never further.

A crash, `process::exit` without `flush_pending_metadata`, or a signal
loses at most the queued bytes; a process that exits normally or drops
its backends loses nothing. `HostBackend::with_write_behind(false)` gives
synchronous writes, as does a failure to spawn the thread (logged). A
differential test runs the same random sequence of creates, overlapping writes, reads, stats, truncations, flushes,
cleanups, removals, listings and volume queries with write-behind on and
off and compares every result and the final host bytes
(`crates/runtime/vfs/src/host_write_behind_tests.rs`). `cargo test -p vfs
--release bench_write_behind -- --ignored --nocapture` measures the
calling thread's time per 256 KiB write either way, with the caller busy
for 0, 1 or 5 ms between writes. On a heavily loaded host (load average
about 130) the median went from 300 to 600 µs synchronously to 33 to
60 µs queued with room between writes; back to back, backpressure holds
both to the host's rate (about 270 µs each).

Persistent metadata lives under `metadata/PartitionN`, outside guest-visible
mounts. Each lowercase path component is hex encoded as a metadata-directory
component; a `record` contains `HVF1`, the little-endian u32 attributes and
four little-endian u64 times, 40 bytes total. Missing/truncated/invalid records
fall back to host defaults; a later successful metadata write repairs them.
Updates write and fsync a staging file, rename it over the record, and fsync
the containing directory. New metadata directories are synced through their
parents. Removal unlinks the file (or, after removing known litter, rmdirs
the directory) **before** retiring metadata. ENOTEMPTY, permission errors,
or litter cleanup failure therefore leave the entry's attributes/timestamps
intact, durable record and pending write metadata alike. If a successful
unlink or rmdir is followed by a metadata retirement error, the deletion
succeeds and logs a warning; the orphan record is never read. Creation clears any orphaned metadata subtree
before adding a new host entry, so later recreation cannot inherit stale
attributes or child records. Post-removal parent-sync errors are also logged
without undoing a successful namespace operation. Preserve both `disk/` and `metadata/` when
backing up storage.
Out-of-band removal/recreation of a host file can retain obsolete metadata.

Host allocation is emulated in 16 KiB units, including one unit per directory
below the volume root. Growth holes read as zero but count toward logical
allocation even if the host stores them sparsely. Volume capacities are
emulator defaults: Partition1 5 GiB, Partition2 500 MiB and each cache 750 MiB;
`HostBackend::with_volume` customizes label, serial and capacity. Growth
checks logical quota and FATX maximum size before writing; host ENOSPC is
also mapped to DiskFull. Available space is cached from eligible host entries and
does not promise the host has that much physical free space. The root,
metadata, FAT structures, fragmentation and directory growth are not charged
as a physical FATX allocator would charge them.

## File objects and NT behavior

The numeric access/share/disposition constants agree with
[nxdk's public Xbox kernel header](https://github.com/XboxDev/nxdk/blob/master/lib/xboxkrnl/xboxkrnl.h).
`OpenOptions::from_nt` expands generic rights and decodes supported creation
flags. It validates share bits, mutually exclusive directory/type and sync
options, SYNCHRONIZE for synchronous opens, and DELETE for delete-on-close.
FILE_WRITE_THROUGH (`0x2`) and FILE_NO_INTERMEDIATE_BUFFERING (`0x8`) are
accepted as hints without changing the public options struct. Kernel code
must enforce guest alignment if required; VFS byte-slice operations remain
buffered/synchronous, and these hints do not promise per-write host durability.
Use `flush` for durable completion. Open-by-ID and reparse-point flags remain
`NotSupported`. Accepted hints do not provide asynchronous I/O, host ACLs,
caching guarantees or a guest event implementation.

| Disposition | Missing leaf | Existing file | Success information |
| --- | --- | --- | --- |
| FILE_SUPERSEDE (0) | Create | Truncate and replace attributes | Created (2) / Superseded (0) |
| FILE_OPEN (1) | ObjectNameNotFound | Open unchanged | Opened (1) |
| FILE_CREATE (2) | Create | ObjectNameCollision | Created (2) |
| FILE_OPEN_IF (3) | Create | Open unchanged | Created (2) / Opened (1) |
| FILE_OVERWRITE (4) | ObjectNameNotFound | Truncate, add requested attributes | Overwritten (3) |
| FILE_OVERWRITE_IF (5) | Create | Truncate, add requested attributes | Created (2) / Overwritten (3) |

A missing parent always fails; VFS does not implicitly create trees. Create
options select the directory/file type, not the DIRECTORY attribute input.
Truncating a directory fails. Overwrite requires WRITE. Supersede of an
existing file no longer requires the caller to explicitly request DELETE.
The operation checks implied DELETE against existing share reservations,
then grants only the requested handle rights. Supersede of a missing leaf
creates it without implied deletion. This follows FastFat's distinction
between operation access and granted access, not a claim that retail Xbox
behavior has been measured: `FatOpenExistingFcb` records `AddedAccess`, adds
DELETE for supersede, checks access/sharing, then clears `AddedAccess` before
`IoSetShareAccess`; `FatOpenExistingFile` follows the same pattern.
See [Microsoft FastFat create.c, pinned revision](https://github.com/microsoft/Windows-driver-samples/blob/96eb96dfb613e4c745db6bd1f53a92fe7e2290fc/filesys/fastfat/create.c#L3332)
and [removal of implied access before share registration](https://github.com/microsoft/Windows-driver-samples/blob/96eb96dfb613e4c745db6bd1f53a92fe7e2290fc/filesys/fastfat/create.c#L3775).
A WRITE-only supersede therefore succeeds when deletion is permitted by
existing shares, but its resulting handle cannot set delete disposition.
The previous explicit-DELETE rejection was too strict. Supersede is still
replacement in place, without a detached old inode or ACL replacement.

Each independent open acquires a share reservation for data-read/execute,
data-write/append and delete access. Existing share flags must permit the new
access, and the new share flags must permit every existing access. Checks
happen **before truncation**. Metadata-only handles do not acquire data
read/write reservations. All drive aliases use the resolved mount and
lowercase volume path as identity. Successful opens return a `Handle` and
`CreateInformation`; the latter is already the numeric IO_STATUS_BLOCK
Information value, not an NTSTATUS.

`duplicate` references the same file object: byte position, search expression,
search cursor and share reservation are shared. A separate `open` creates a
new position/search cursor. `close` frees the reservation when the last
handle to its object closes. Handles are opaque Rust values; kernel handles
must live in the kernel object table. An identifier is valid only in the VFS
instance that created it. Handle values are opaque and never reused within
that VFS instance, including after close. Do not serialize or reconstruct them.

`read`/`write` return the actual transferred byte count. Operations are
synchronous; Current and Absolute offsets update the shared position after
success. APPEND-only rights force writes to EOF regardless of supplied
offset. `Offset::EndOfFile` requests append for a writable handle; it is
invalid for reads. Negative NT sentinel offsets must be translated by the
kernel; ordinary offsets must fit signed LARGE_INTEGER. A nonempty read at
or beyond EOF returns EndOfFile; a read crossing EOF returns the available
prefix successfully. A zero-length read succeeds with zero bytes. EOF resize
does not move the position. Position behavior follows the synchronous model
in [Microsoft's read contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/wdm/nf-wdm-zwreadfile).

`set_delete_pending` requires DELETE and rejects read-only files, volume roots
and nonempty directories. An explicit disposition request becomes pending
immediately and can be cancelled before cleanup. FILE_DELETE_ON_CLOSE instead
records an intent on its file object: other compatible opens are allowed
until its last duplicated handle closes and cleanup runs. Cleanup makes the
pending request sticky while other file objects remain; a different object
cannot cancel that completed cleanup by setting disposition false. Pending
paths reject new opens (including beneath pending directories); existing
handles remain usable until all file objects for the path close. This file-object distinction follows [Microsoft's delete-on-close explanation](https://devblogs.microsoft.com/oldnewthing/20160108-00/?p=92821).
The last close removes the host entry. Final-object cleanup uses
`Backend::cleanup(path, write_access)`. Its default calls `flush` only for
file objects opened with WRITE/APPEND; directory and read-only opens avoid
unconditional sync. HostBackend additionally tracks dirty data and deferred
atime: atime-only cleanup persists its record without syncing file data;
write-capable or data-dirty cleanup syncs file data and persists the record
any write left. Duplicate-handle closes
do not trigger cleanup until the last reference closes. A cleanup-sync
failure is logged to stderr with a `vfs:` prefix and does not turn an already
closed handle into a failed close. Actual removal errors still propagate.
Call `flush` before close to receive a durability failure synchronously.
Session drop is best effort. There is no crash-time delete replay.

Directory enumeration is a sorted, case-insensitive snapshot per file object,
without `.`/`..` entries. The first expression is captured; later expressions
are ignored, including on restart. Restart rebuilds and rewinds the snapshot.
`*` and `?` work case-insensitively; `?` consumes exactly one byte and a dot
is literal. Consequently `ab?` does not match `ab`, and `a*.*` does not match
`abc`. This kernel-facing behavior follows [OXOS's original-Xbox matcher](https://github.com/kooscode/oxos/blob/main/kernel/src/filesystems/hddfs/volume.cpp#L327).
Cxbx [forwards the mask to NtQueryDirectoryFile](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlNt.cpp#L1197),
with a specific `*.*` to `*` rewrite that VFS retains, so `*.*` does include
extensionless names. These implementation sources do not establish broader
Win32 DOS wildcard translation on original hardware. In particular,
[nboxkrnl's FATX driver table](https://github.com/ergo720/nboxkrnl/blob/master/nboxkrnl/io/hdd/fatx.cpp#L78)
registers `IopUnimplementedDeviceRequest` for IRP_MJ_DIRECTORY_CONTROL; that
source has a name validator, but no FATX enumeration/name matcher to cite.
**Kernel integration compatibility note:** VFS's `a*.*` will not match `abc`.
If retail Xbox applies DOS-style semantics that make this a match, the
current VFS mask behavior will miss the file and needs a hardware-backed
matcher correction; desktop forwarding or a reimplementation does not prove
retail behavior. DOS_STAR, DOS_QM and DOS_DOT encodings remain unsupported; callers must not silently translate
ordinary `?` into DOS_QM based on desktop FindFirstFile behavior. An absent
or initially empty expression means `*`. Initial no-match returns
NoSuchFile; exhaustion returns NoMoreFiles. See the expression/cursor model
in [Microsoft's directory query contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntquerydirectoryfile).

Use `peek_directory` to inspect an entry without consuming it, pack the guest
record, then `consume_directory` after it fits. A buffer-too-small retry must
not consume or restart the cursor. `query_directory` combines those calls for
callers needing one complete entry. VFS does not know the guest buffer size,
information class or NextEntryOffset layout. Querying a file as a directory
returns InvalidParameter; using a file as an intermediate path component
returns ObjectPathNotFound (an explicit directory-type open on that same
final file still returns NotADirectory).

## Kernel integration

The kernel depends on `vfs` and translates the NT file exports onto it; the
boot runner configures it (docs/runtime/BOOT.md, `--data`).

`Vfs::set_open_observer` is told about every file (not directory) opened,
under the VFS lock, with `OpenedFile::host`: where its bytes sit on the host
(`Backend::host_file`: an entry's region of the disc image, a hard-disk file,
`None` for a rewritten one). `Vfs::host_file(name)` answers the same for any
name. The controller-speaker sound index uses both to read the maps the game
loads ([playstation-audio.md](../audio/playstation-audio.md)).
`Vfs::set_change_observer` is told when a change is finished: `ChangeKind::Written`
at the last close of a file object that created, overwrote, wrote or
truncated a file (after its cleanup, so the bytes are on the host), and
`ChangeKind::Removed` when a deletion is carried out. Cloud saves watch U:
with it ([live-cloud-saves.md](../live/live-cloud-saves.md)). Basic setup:

```rust
use vfs::{Config, OpenOptions, Offset, Vfs};

let config = Config::for_title(0x4D530064)?;
let mut storage = Vfs::with_disc(config.clone(), "disc/halo2.iso")?;
storage.create_standard_links(&config)?; // host tool only; on the boot path XAPI creates D:
let opened = storage.open(r"D:\default.xbe", OpenOptions::default())?;
let mut header = [0u8; 64];
let transferred = storage.read(opened.handle, Offset::Current, &mut header)?;
storage.close(opened.handle)?;
```

| Xbox export / operation | Rust API |
| --- | --- |
| NtCreateFile / NtOpenFile | `OpenOptions::from_nt`, `open` / `open_relative` |
| NtReadFile / NtWriteFile | `read` / `write`, translating ByteOffset to `Offset` |
| NtClose / NtDuplicateObject | `close` / `duplicate` |
| NtQueryInformationFile | `query_information` returns basic/standard/position/access/name data |
| NtQueryFullAttributesFile | `query_path` |
| NtSetInformationFile EOF / position / basic / disposition | `set_end_of_file`, `set_position`, `set_basic_information`, `set_delete_pending` |
| NtQueryDirectoryFile | `peek_directory` + `consume_directory`, or `query_directory` |
| NtQueryVolumeInformationFile | `query_volume_information` |
| NtFlushBuffersFile | `flush` |
| IoCreateSymbolicLink / IoDeleteSymbolicLink | `namespace.create_symbolic_link` / `delete_symbolic_link` |

The kernel validates guest buffers **before** operations with effects (file
creation, I/O, cursor consumption), maintains guest object references, and
marshals Xbox-width structures. Validate per-information-class rights at that
boundary; `query_information` is a combined typed snapshot, not an unchecked
implementation of every class. `set_basic_information` replaces a full
`BasicInfo`; merge NT zero/-1 unchanged-time sentinels and unchanged attributes
with current values before calling it. Pack Xbox names as byte strings, not
Windows UTF-16 names. Async completion, IRPs, events/APCs, notifications, raw
sector/device IOCTLs, file IDs, rename, EAs, oplocks,
byte-range locks, volume formatting and ACLs are not implemented here.

`Backend::cleanup(&mut self, path: &str, write_access: bool) -> Result<()>`
handles deferred metadata independently of `release_allocation(path, owner)`.
The initial-allocation methods receive that same owner identity; backend
wrappers implementing reservations must forward it and its release to their
inner backend. Default methods reject unsupported nonzero reservations before
mutation. VFS logs cleanup/release errors separately from close. Existing
open/close signatures, OpenOptions and Handle are unchanged; allocated-open
variants carry the initial allocation hint.
`set_basic_information` remains full replacement and must be merged by the
kernel; `Handle` remains opaque and nonreusable. Rename is still unavailable.
FileRenameInformation within a volume is a useful next addition for XAPI
MoveFile/save handling, but should be designed with handle/share identity,
replacement rules and metadata movement together; it is not emulated by
unlink/copy in this patch.

Every failure is a typed `vfs::Error`; `error.ntstatus()` is the numeric status
for the guest. Success is STATUS_SUCCESS (`0`), with byte count or create
information written separately. Storage errors include:

| Error | NTSTATUS |
| --- | --- |
| InvalidHandle / InvalidParameter | `C0000008` / `C000000D` |
| NoSuchFile / EndOfFile | `C000000F` / `C0000011` |
| AccessDenied | `C0000022` |
| ObjectNameInvalid / ObjectNameNotFound / ObjectNameCollision | `C0000033` / `C0000034` / `C0000035` |
| ObjectPathNotFound / SharingViolation / DeletePending | `C000003A` / `C0000043` / `C0000056` |
| DiskFull / MediaWriteProtected | `C000007F` / `C00000A2` |
| FileIsADirectory / NotSupported | `C00000BA` / `C00000BB` |
| DirectoryNotEmpty / FileCorrupt / NotADirectory | `C0000101` / `C0000102` / `C0000103` |
| IoDeviceError / ReparsePointNotResolved / FileTooLarge | `C0000185` / `C0000280` / `C0000904` |
| NoMoreFiles (warning severity) | `80000006` |

Host errors are mapped by category; unexpected I/O becomes IoDeviceError,
malformed XISO becomes FileCorrupt. The typed error does not retain a private
host path. Partial host writes return their transferred count. A metadata
failure after a data write can report failure after data has changed; this
backend is not a journaled filesystem. `flush` explicitly persists deferred
atime and syncs file data. Close performs conditional cleanup as described
above. A successful create whose final host-parent sync fails still reports
success (the entry and metadata already exist) and logs a durability warning
instead of returning an error that encourages a duplicate create. Earlier
metadata setup errors still fail creation and attempt rollback. Sidecar
records/directories are synced, but file data and logical metadata still
are not a single crash-atomic transaction. Failed host deletion after metadata retirement can leave intact
data with default logical attributes; out-of-band replacement may also retain
old metadata until it is explicitly removed.

## XAPI save containers

The filesystem permits XAPI's save layout without implementing XCreateSaveGame,
XFindFirstSaveGame, XDeleteSaveGame or save signing. The `xapi` crate's opt-in
native save games (docs/runtime/xapi.md, "Opt-in native save games") do implement
them over this VFS: a save directory is the 12-hex-digit name hash holding
`SaveMeta.xbx`, in the host `UDATA` title directory. T: maps title data such as
configuration/DLC, and U: maps user saves. Each title owns an eight-hex-digit
directory in both areas. A representative layout is:

```text
E:\
  TDATA\4D530064\                 (T:\)
    <title-defined files>
  UDATA\4D530064\                 (U:\)
    TitleMeta.xbx
    TitleImage.xbx
    SaveImage.xbx                 (optional shared save image)
    <save-directory>\
      SaveMeta.xbx
      SaveImage.xbx               (optional per-save override)
      <game-defined payload>
```

TitleMeta.xbx describes the title (`TitleName=...`); SaveMeta.xbx describes a
save (`Name=...`). Metadata is text, commonly UTF-16LE; a save scanner should
also account for ASCII and localization sections, as demonstrated by the
[OGXBL homebrew metadata reader](https://github.com/OGXBL/XBLCloudGameSave/blob/main/README.md#save-metadata).
TitleImage.xbx and SaveImage.xbx are Xbox texture resources. A save-specific
image overrides the shared image in the title directory. This organization
and image precedence are described in the
[Xbox savegame format notes](https://xboxdevwiki.net/Xbox_Savegame_System).
VFS only creates the TDATA/UDATA title roots: it does not invent save IDs,
write metadata/images, decode textures, verify signatures or erase existing
containers. These files are ordinary byte payloads to the filesystem.

## Validation

`cargo test -p vfs` builds all its own fixtures. Host tests create private
RAII temp directories under TMPDIR and delete them afterwards; synthetic
XISO tests build a small in-memory image, including a disc name longer than
42 bytes. Tests exercise every disposition for existing/missing files,
case lookup/collisions, aliases, shares, directory search/restart/retry,
duplicate positions, EOF/append/growth, persistent attributes/times, deletion,
quota, directory count limits and host link rejection. Review regressions add
readonly create/existing-handle rights, buffering hints, damaged records and
unrepresentable neighbors, host litter, metadata retirement failure, deferred
atime and scan-count assertions, cleanup-time deletion, supersede permissions,
original-Xbox punctuation, mask semantics, invalid disc names and low-level
status/configuration cases. Re-review tests cover litter-only rmdir through
guest handles, preserved attributes on failed rmdir, non-followed litter
symlinks, orphan metadata on recreation, conditional cleanup/close errors,
and injected post-create sync failure. No game data is
needed by the default suite.

The ignored `real_disc_lists_d_root` test opens the user's existing
`disc/halo2.iso` (or H2_DISC) and prints only the top-level names. Run with
`cargo test -p vfs real_disc_lists_d_root -- --ignored --nocapture`. It does
not extract anything. Builds can use `CARGO_TARGET_DIR="$TMPDIR/vfs-target"`
to keep scratch output outside the shared checkout.
