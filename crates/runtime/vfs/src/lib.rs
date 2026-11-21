//! Xbox object-manager storage, independent of guest memory and kernel ABI packing.
//!
//! See `docs/runtime/storage.md` for mappings, supported semantics, and the integration contract.
#![deny(unsafe_code)]

mod backend;
mod campaign_maps;
mod content_maps;
mod dir_disc;
mod disc;
mod host;
mod film_map;
#[cfg(unix)]
pub use film_map::DetachedMapReader;
pub use film_map::{FilmMap, FilmMapSource, ImmutableRead};
mod map_override;
mod packages;
mod path;
mod rewrite;
pub mod save_meta;
pub mod sim_store;
mod state;
mod status;
mod types;
mod variants;
mod write_behind;

pub use backend::Backend;
pub use campaign_maps::CampaignMapsBackend;
pub use content_maps::{ContentMap, ContentMaps};
pub use dir_disc::{AnyDisc, DirDiscBackend, open_disc, open_disc_with_mapping};
pub use disc::DiscBackend;
pub use host::{HostBackend, METADATA_STALE, flush_pending_metadata};
pub use map_override::{MapOverrideBackend, MapOverrideNotice};
pub use packages::{PackageBackend, Scope as PackageScope};
pub use rewrite::{Chain, MAX_REWRITE_BYTES, Rewrite, RewriteBackend};
pub use sim_store::SimStoreBackend;
pub use path::{Namespace, validate_fatx_name, validate_fatx_path, wildcard_matches};
pub use state::{SavedFile, StateError, VfsState};
pub use status::{Error, Result};
pub use types::*;
pub use variants::{ContentVariants, SandboxChoice, VariantFile, VariantFiles, VariantPair};

use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};

pub const CDROM_DEVICE: &str = "\\Device\\CdRom0";
pub const DATA_DEVICE: &str = "\\Device\\Harddisk0\\Partition1";

/// Directory under a title's `T:` that holds installed content (downloaded or
/// installed from a disc), one subdirectory per offer.
pub const CONTENT_DIR: &str = "$c";

/// Name of a content package's directory: the 64-bit offer ID as 16 lowercase
/// hex digits, the `%016I64x` the title formats it with.
pub fn content_dir_name(offer_id: u64) -> String {
    format!("{offer_id:016x}")
}

/// An offer ID for `title_id`: the title ID in the high half, the title's own
/// content number in the low half.
pub fn offer_id(title_id: u32, content: u32) -> u64 {
    (u64::from(title_id) << 32) | u64::from(content)
}

#[derive(Debug, Clone)]
pub struct Config {
    pub data_root: PathBuf,
    pub title_id: u32,
    /// Retail utility area is selected from cache partitions 3..=5.
    pub utility_partition: u8,
    /// Additional logical space per cache partition for native campaign slots.
    pub extra_cache_bytes: u32,
    /// Converted maps on the data partition served by a sandbox choice
    /// ([`VariantFiles`]); empty unless the boot registers some.
    pub variants: ContentVariants,
    /// Independent paths for installed multiplayer packages.
    pub content_maps: ContentMaps,
    /// Map packages (normally `<data>/maps`) served over the Multiplayer
    /// Map Pack's maps installed on the data partition
    /// ([`PackageBackend::over_map_pack`], docs/game/maps.md "Importing a
    /// mod"); `None` serves the partition as it is.
    pub map_packages: Option<PathBuf>,
    /// Use read-only mappings for the immutable DVD, when possible.
    pub mmap_disc: bool,
    /// Serve eligible guest reads by installing private file pages.
    pub map_guest_reads: bool,
    /// Verified server copies, keyed by DVD filename or installed MapId.
    pub server_maps: Option<PathBuf>,
    /// Sim stores ([`sim_store`]): serve `maps\<map>.map` from this
    /// directory's `<map>.h2store` files instead of the disc. Maps with no
    /// store come from the disc as usual.
    pub sim_store: Option<PathBuf>,
}
impl Config {
    pub fn new(data_root: impl Into<PathBuf>, title_id: u32) -> Self {
        Self {
            data_root: data_root.into(),
            title_id,
            utility_partition: 5,
            extra_cache_bytes: 0,
            variants: ContentVariants::default(),
            content_maps: ContentMaps::default(),
            map_packages: None,
            mmap_disc: false,
            map_guest_reads: false,
            server_maps: None,
            sim_store: None,
        }
    }
    /// Host directory behind `T:`: `<data_root>/disk/Partition1/TDATA/<TITLE>`.
    pub fn title_data_dir(&self) -> PathBuf {
        self.data_root
            .join("disk")
            .join("Partition1")
            .join("TDATA")
            .join(format!("{:08X}", self.title_id))
    }
    /// Host directory of one installed content package, `T:\$c\<offer id>`.
    /// See [`content_dir_name`] and docs/game/map-pack.md.
    pub fn content_dir(&self, offer_id: u64) -> PathBuf {
        self.title_data_dir()
            .join(CONTENT_DIR)
            .join(content_dir_name(offer_id))
    }
    /// The layout under the host's default data root:
    /// [`content::store::app_data_dir`].
    pub fn for_title(title_id: u32) -> Result<Self> {
        let root = content::store::app_data_dir(content::store::DataHost::CURRENT, |name| std::env::var_os(name))
            .ok_or(Error::InvalidParameter)?;
        Ok(Self::new(root, title_id))
    }
}

/// Successful NtReadFile bytes for one mounted machine, including map loads.
#[derive(Default, Clone, Copy, Debug)]
pub struct GuestReadBytes {
    pub mapped: u64,
    pub copied: u64,
}

struct Mount {
    device: String,
    backend: Box<dyn Backend>,
}
struct Enumeration {
    entries: Vec<DirectoryEntry>,
    next: usize,
    pattern: String,
}
struct OpenFile {
    mount: usize,
    path: String,
    key: String,
    options: OpenOptions,
    position: u64,
    enumeration: Option<Enumeration>,
    delete_requested: bool,
    directory: bool,
    /// Created, overwritten, written or truncated through this object:
    /// reported as [`ChangeKind::Written`] at its last close.
    changed: bool,
}

/// Told about every file (not directory) the VFS opens; see
/// [`Vfs::set_open_observer`].
pub type OpenObserver = Box<dyn FnMut(&OpenedFile) + Send>;

/// Told about every file written and every file or directory removed; see
/// [`Vfs::set_change_observer`].
pub type ChangeObserver = Box<dyn FnMut(&Changed) + Send>;

/// One storage session and its NT file objects. Use one Vfs for all guest threads;
/// put a mutex around it in the kernel. Share checks span every alias within this instance.
#[derive(Default)]
pub struct Vfs {
    map_guest_reads: bool,
    read_copy_reasons: HashSet<&'static str>,
    guest_reads: GuestReadBytes,
    extra_cache_bytes: u32,
    pub namespace: Namespace,
    mounts: Vec<Mount>,
    content_maps: ContentMaps,
    handles: BTreeMap<u64, u64>,
    objects: BTreeMap<u64, OpenFile>,
    pending: HashSet<(usize, String)>,
    cleanup_pending: HashSet<(usize, String)>,
    next_handle: u64,
    observer: Option<OpenObserver>,
    change_observer: Option<ChangeObserver>,
}
impl Vfs {
    /// Additional logical bytes across the three host-backed cache partitions.
    pub fn extra_disk_bytes(&self) -> u64 {
        3 * u64::from(self.extra_cache_bytes)
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// Standard hard disk layout: mounts the device objects only (`\\Device\\Harddisk0\\PartitionN`)
    /// and creates no drive-letter links. The kernel provides devices; XAPI creates D:/T:/U:/Z: itself
    /// and treats an existing T: as a fatal collision. Host tools that want the links call
    /// [`Vfs::create_standard_links`]. Callers may mount CDROM_DEVICE separately for a disc-less session.
    pub fn with_storage(config: Config) -> Result<Self> {
        if !(3..=5).contains(&config.utility_partition) {
            return Err(Error::InvalidParameter);
        }
        // Before anything opens U:'s files: a profile whose SaveMeta.xbx
        // was left damaged is made visible again (docs/runtime/xapi.md
        // "Damaged save metadata").
        let udata = config
            .data_root
            .join("disk")
            .join("Partition1")
            .join("UDATA")
            .join(format!("{:08X}", config.title_id));
        match save_meta::repair_saves(&udata, &config.data_root) {
            Ok(done) => {
                for repair in done {
                    eprintln!("vfs: {repair}");
                }
            }
            Err(e) => eprintln!("vfs: damaged saves not repaired: {e}"),
        }
        let mut vfs = Self::new();
        vfs.map_guest_reads = config.map_guest_reads;
        vfs.extra_cache_bytes = config.extra_cache_bytes;
        vfs.content_maps = config.content_maps.clone();
        for partition in 1..=5 {
            let name = format!("Partition{partition}");
            let mut backend = HostBackend::new(
                config.data_root.join("disk").join(&name),
                config.data_root.join("metadata").join(&name),
            )?
            .with_volume(
                name.clone(),
                partition,
                match partition {
                    1 => 5 * 1024 * 1024 * 1024,
                    2 => 500 * 1024 * 1024,
                    _ => 750 * 1024 * 1024 + u64::from(config.extra_cache_bytes),
                },
            );
            if partition == 1 {
                for base in ["TDATA", "UDATA"] {
                    for dir in [base.to_owned(), format!("{base}\\{:08X}", config.title_id)] {
                        match backend.stat(&dir) {
                            Ok(m) if m.is_directory() => {}
                            Ok(_) => return Err(Error::NotADirectory),
                            Err(Error::ObjectNameNotFound) => backend.create(&dir, true, 0)?,
                            Err(e) => return Err(e),
                        }
                    }
                }
            }
            let device = format!("\\Device\\Harddisk0\\{name}");
            if partition == 1 {
                let variants = config.variants.clone();
                match &config.map_packages {
                    Some(root) => {
                        let library = content::Library::open(root);
                        let packages =
                            PackageBackend::over_map_pack(backend, library, config.title_id);
                        vfs.mount(&device, VariantFiles::new(packages, variants))?
                    }
                    None => vfs.mount(
                        &device,
                        VariantFiles::new(backend, variants).with_packages(
                            config.content_maps.clone(),
                            &config.data_root.join("maps"),
                            config.server_maps.clone(),
                        ),
                    )?,
                }
            } else {
                vfs.mount(&device, backend)?;
            }
        }
        Ok(vfs)
    }
    /// Creates D:, T:, U: and Z: for host tools that have no XAPI. Not for the boot or game path:
    /// XAPI creates these links and fails if T: already exists. Pass the `Config` given to
    /// `with_storage`; errors with `ObjectNameCollision` if any link exists already. Not atomic: links
    /// created before a failure are not rolled back.
    pub fn create_standard_links(&mut self, config: &Config) -> Result<()> {
        if !(3..=5).contains(&config.utility_partition) {
            return Err(Error::InvalidParameter);
        }
        for (drive, target) in [
            ('D', CDROM_DEVICE.to_owned()),
            (
                'Z',
                format!("\\Device\\Harddisk0\\Partition{}", config.utility_partition),
            ),
            (
                'T',
                format!("{DATA_DEVICE}\\TDATA\\{:08X}", config.title_id),
            ),
            (
                'U',
                format!("{DATA_DEVICE}\\UDATA\\{:08X}", config.title_id),
            ),
        ] {
            self.namespace
                .create_symbolic_link(&format!("\\??\\{drive}:"), &target)?;
        }
        Ok(())
    }
    pub fn with_disc(config: Config, disc: impl AsRef<std::path::Path>) -> Result<Self> {
        // Fail a bad image before creating persistent storage directories.
        // A directory holding the disc's files serves as well (tvOS).
        let disc = open_disc_with_mapping(disc, config.mmap_disc)?;
        let mut vfs = Self::with_storage(config)?;
        vfs.mount(CDROM_DEVICE, disc)?;
        Ok(vfs)
    }
    /// [`Vfs::with_disc`], with some of the disc's files served rewritten
    /// (see [`RewriteBackend`]). The image itself is never modified.
    pub fn with_disc_rewrite(
        config: Config,
        disc: impl AsRef<std::path::Path>,
        rewrite: impl Rewrite + 'static,
    ) -> Result<Self> {
        let disc = open_disc_with_mapping(disc, config.mmap_disc)?;
        let mut vfs = Self::with_storage(config)?;
        vfs.mount(CDROM_DEVICE, RewriteBackend::new(disc, rewrite))?;
        Ok(vfs)
    }
    /// Storage plus `disc` as the DVD: [`Vfs::with_disc`] for a disc backend
    /// already built (for example one wrapped in [`MapOverrideBackend`]).
    pub fn with_disc_backend(config: Config, disc: impl Backend + 'static) -> Result<Self> {
        let mut vfs = Self::with_storage(config)?;
        vfs.mount(CDROM_DEVICE, disc)?;
        Ok(vfs)
    }
    pub fn mount(&mut self, device: &str, backend: impl Backend + 'static) -> Result<()> {
        let device = path::object_name(device)?;
        if !device.to_ascii_lowercase().starts_with("\\device\\") {
            return Err(Error::ObjectNameInvalid);
        }
        if self.mounts.iter().any(|m| {
            path::prefix(&device, &m.device).is_some() || path::prefix(&m.device, &device).is_some()
        }) {
            return Err(Error::ObjectNameCollision);
        }
        self.mounts.push(Mount {
            device,
            backend: Box::new(backend),
        });
        Ok(())
    }
    /// Quick-format one mounted device (`\Device\Harddisk0\PartitionN`, exact name). For XAPI's
    /// cache volumes, which retail formats from raw sectors; here the backend empties itself.
    /// Fails with `SharingViolation`, changing nothing, while any file object on the volume is open.
    /// Any other failure also changes nothing: a failed format leaves the volume as it was.
    /// Symbolic links are left alone.
    pub fn format_device(&mut self, device: &str) -> Result<()> {
        let device = path::object_name(device)?;
        let index = self
            .mounts
            .iter()
            .position(|m| m.device.eq_ignore_ascii_case(&device))
            .ok_or(Error::ObjectPathNotFound)?;
        if self.objects.values().any(|f| f.mount == index)
            || self.pending.iter().any(|(m, _)| *m == index)
        {
            return Err(Error::SharingViolation);
        }
        self.mounts[index].backend.format()
    }
    /// The mounted device `name` resolves into, after every symbolic link (including links into
    /// a subdirectory of a volume). Identifies the volume a drive letter really names.
    pub fn device_of(&self, name: &str) -> Result<&str> {
        let (index, _) = self.resolve(name)?;
        Ok(&self.mounts[index].device)
    }
    fn resolve(&self, name: &str) -> Result<(usize, String)> {
        let resolved = self.namespace.resolve(name)?;
        for (index, mount) in self.mounts.iter().enumerate() {
            if let Some(rest) = path::prefix(&resolved, &mount.device) {
                path::components(rest)?;
                return Ok((index, rest.to_owned()));
            }
        }
        Err(Error::ObjectPathNotFound)
    }
    fn object(&self, handle: Handle) -> Result<&OpenFile> {
        let id = self.handles.get(&handle.0).ok_or(Error::InvalidHandle)?;
        self.objects.get(id).ok_or(Error::InvalidHandle)
    }
    fn object_mut(&mut self, handle: Handle) -> Result<&mut OpenFile> {
        let id = self.handles.get(&handle.0).ok_or(Error::InvalidHandle)?;
        self.objects.get_mut(id).ok_or(Error::InvalidHandle)
    }
    fn allocate_handle(&mut self) -> Result<u64> {
        self.next_handle = self
            .next_handle
            .checked_add(1)
            .ok_or(Error::InvalidHandle)?;
        Ok(self.next_handle)
    }
    pub fn open(&mut self, name: &str, options: OpenOptions) -> Result<OpenResult> {
        self.open_allocated(name, options, 0)
    }
    /// Calls `observer` after every successful open of a file (not a
    /// directory), with the guard on this `Vfs` held: it must be quick and
    /// must not call back into the VFS. The controller-speaker sound index
    /// uses it to see which maps the game loads (docs/audio/playstation-audio.md).
    /// `None` removes it.
    pub fn set_open_observer(&mut self, observer: Option<OpenObserver>) {
        self.observer = observer;
    }
    /// Calls `observer` when a change is finished, with the guard on this
    /// `Vfs` held (quick, and no calls back into the VFS):
    /// [`ChangeKind::Written`] at the last close of a file object that
    /// created, overwrote, wrote or truncated a file (after its cleanup,
    /// so the bytes are on the host), and [`ChangeKind::Removed`] when a
    /// deletion is carried out. Cloud saves watch U: with it
    /// (docs/live/live-cloud-saves.md). `None` removes it.
    pub fn set_change_observer(&mut self, observer: Option<ChangeObserver>) {
        self.change_observer = observer;
    }
    fn report_change(&mut self, mount: usize, path: &str, kind: ChangeKind) {
        if let Some(observe) = self.change_observer.as_mut() {
            observe(&Changed {
                device: self.mounts[mount].device.clone(),
                path: path.to_owned(),
                kind,
            });
        }
    }
    /// Where the file `name` (any object name or link) sits on the host, as
    /// [`Backend::host_file`] says; `None` if it is not one contiguous host
    /// region or does not exist.
    pub fn host_file(&mut self, name: &str) -> Option<HostFile> {
        let (mount, path) = self.resolve(name).ok()?;
        self.mounts[mount].backend.host_file(&path)
    }
    /// The backing for `name` as mounted, when it can be read outside the VFS.
    pub fn file_source(&mut self, name: &str) -> Option<FileSource> {
        let (mount, path) = self.resolve(name).ok()?;
        self.mounts[mount].backend.file_source(&path)
    }
    /// The entries of the directory `name` (any object name or link), as
    /// its backend lists them: for host-side readers that look for a file
    /// by its contents (a map by its header's name), not for the guest.
    pub fn list_directory(&mut self, name: &str) -> Result<Vec<DirectoryEntry>> {
        let (mount, path) = self.resolve(name)?;
        self.mounts[mount].backend.read_directory(&path)
    }
    /// Which map exactly `name` serves, when it isn't the retail disc map:
    /// a map package's `MapId`, or a whole-file override's. `None` means
    /// retail (or not a map). See [`Backend::map_id`].
    pub fn map_id(&mut self, name: &str) -> Option<content::MapId> {
        let (mount, path) = self.resolve(name).ok()?;
        self.mounts[mount].backend.map_id(&path)
    }
    /// The device path of an installed package with this short name, when unique.
    pub fn content_map_path(&self, name: &str) -> Option<String> {
        let path = self.content_maps.path_for_name(name)?;
        Some(format!("{DATA_DEVICE}\\{path}"))
    }

    /// The original map name used to group matches by base in telemetry.
    pub fn content_base_name(&self, name: &str) -> Option<String> {
        let path = self.content_maps.path_for_name(name)?;
        Some(self.content_maps.get(&path)?.package.manifest.base.map)
    }

    /// Look again at map packages (`<data>/maps`) on every mount, after
    /// one was installed or edited while the game runs: a map whose
    /// package changed is resolved again at its next open; a file open now
    /// keeps what it had. See [`Backend::rescan`]. The packages backend
    /// also notices changes by itself on each map open; this is for a
    /// writer that knows it just changed something.
    pub fn rescan_maps(&mut self) {
        for m in &mut self.mounts {
            m.backend.rescan();
        }
    }
    /// Whether the map `name` would now be served differently than at its
    /// last open (see [`Backend::map_changed`]); looks at the maps folder
    /// first.
    pub fn map_changed(&mut self, name: &str) -> bool {
        let Ok((mount, path)) = self.resolve(name) else {
            return false;
        };
        self.mounts[mount].backend.map_changed(&path)
    }
    /// Whether a first look at `name` would resolve a map package for it,
    /// reading its whole base with this `Vfs` held (see
    /// [`Backend::resolves_on_use`]).
    pub fn resolves_on_use(&mut self, name: &str) -> bool {
        let Ok((mount, path)) = self.resolve(name) else {
            return false;
        };
        self.mounts[mount].backend.resolves_on_use(&path)
    }
    /// NtCreateFile's initial allocation hint, applied only on create/overwrite.
    /// It reserves logical volume capacity without changing the initial EOF.
    pub fn open_allocated(
        &mut self,
        name: &str,
        mut options: OpenOptions,
        allocation: u64,
    ) -> Result<OpenResult> {
        if name.ends_with('\\') {
            if options.directory == Some(false) {
                return Err(Error::InvalidParameter);
            }
            options.directory = Some(true);
        }
        self.open_resolved(self.resolve(name)?, options, allocation)
    }
    /// OBJECT_ATTRIBUTES.RootDirectory support. Absolute names must use open().
    pub fn open_relative(
        &mut self,
        directory: Handle,
        name: &str,
        options: OpenOptions,
    ) -> Result<OpenResult> {
        self.open_relative_allocated(directory, name, options, 0)
    }
    pub fn open_relative_allocated(
        &mut self,
        directory: Handle,
        name: &str,
        options: OpenOptions,
        allocation: u64,
    ) -> Result<OpenResult> {
        path::components(name)?;
        let root = self.object(directory)?;
        let (mount, root_path) = (root.mount, root.path.clone());
        if !self.mounts[mount].backend.stat(&root_path)?.is_directory() {
            return Err(Error::NotADirectory);
        }
        let joined = if root_path.is_empty() {
            name.to_owned()
        } else if name.is_empty() {
            root_path
        } else {
            format!("{root_path}\\{name}")
        };
        self.open_resolved((mount, joined), options, allocation)
    }
    fn open_resolved(
        &mut self,
        (mount, path): (usize, String),
        mut options: OpenOptions,
        allocation: u64,
    ) -> Result<OpenResult> {
        options.access = Access::from_nt(options.access.0)?;
        if options.share.0 & !7 != 0 || options.attributes & !(FATX_ATTRIBUTES | ATTR_NORMAL) != 0 {
            return Err(Error::InvalidParameter);
        }
        if options.delete_on_close
            && (path.is_empty()
                || options.attributes & ATTR_READONLY != 0
                || !options.access.contains(Access::DELETE))
        {
            return Err(Error::AccessDenied);
        }
        let key = path.to_ascii_lowercase();
        if self
            .pending
            .iter()
            .any(|(m, p)| *m == mount && path::prefix(&key, p).is_some())
        {
            return Err(Error::DeletePending);
        }
        let overwriting = matches!(
            options.disposition,
            CreateDisposition::Supersede
                | CreateDisposition::Overwrite
                | CreateDisposition::OverwriteIf
        );
        if overwriting && options.directory == Some(true) {
            return Err(Error::InvalidParameter);
        }
        if matches!(
            options.disposition,
            CreateDisposition::Overwrite | CreateDisposition::OverwriteIf
        ) && !options.access.contains(Access::WRITE)
        {
            return Err(Error::AccessDenied);
        }
        let existing = match self.mounts[mount].backend.stat(&path) {
            Ok(m) => Some(m),
            Err(Error::ObjectNameNotFound) => None,
            Err(e) => return Err(e),
        };
        if let Some(info) = &existing {
            if options.disposition == CreateDisposition::Create {
                return Err(Error::ObjectNameCollision);
            }
            if options.directory == Some(true) && !info.is_directory() {
                return Err(Error::NotADirectory);
            }
            if options.directory == Some(false) && info.is_directory() {
                return Err(Error::FileIsADirectory);
            }
            if overwriting && info.is_directory() {
                return Err(Error::FileIsADirectory);
            }
        } else if matches!(
            options.disposition,
            CreateDisposition::Open | CreateDisposition::Overwrite
        ) {
            return Err(Error::ObjectNameNotFound);
        }
        let read_only = self.mounts[mount].backend.read_only();
        if read_only
            && (existing.is_none()
                || overwriting
                || options.delete_on_close
                || options.access.0
                    & (Access::WRITE.0
                        | Access::APPEND.0
                        | Access::WRITE_ATTRIBUTES.0
                        | Access::DELETE.0)
                    != 0)
        {
            return Err(Error::MediaWriteProtected);
        }
        if existing
            .as_ref()
            .is_some_and(|info| info.basic.attributes & ATTR_READONLY != 0)
            && (options.access.0 & (Access::WRITE.0 | Access::APPEND.0 | Access::DELETE.0) != 0
                || overwriting)
        {
            return Err(Error::AccessDenied);
        }
        // Bidirectional checks: old share must permit new access, and new share must permit old access.
        // Overwrite/supersede also modify data even when a caller passes unusual rights.
        for old in self
            .objects
            .values()
            .filter(|f| f.mount == mount && f.key == key)
        {
            let checked_access =
                if existing.is_some() && options.disposition == CreateDisposition::Supersede {
                    options.access | Access::DELETE
                } else {
                    options.access
                };
            if checked_access.sharing() & !old.options.share.0 != 0
                || old.options.access.sharing() & !options.share.0 != 0
            {
                return Err(Error::SharingViolation);
            }
        }
        if options.delete_on_close
            && existing.as_ref().is_some_and(Metadata::is_directory)
            && !self.mounts[mount].backend.read_directory(&path)?.is_empty()
        {
            return Err(Error::DirectoryNotEmpty);
        }
        // Allocate the identity before the backend reserves capacity. Duplicate
        // handles keep this object identity until their last close.
        let handle = self.allocate_handle()?;
        let opened = (|| {
            Ok(if existing.is_some() {
                if overwriting {
                    self.mounts[mount]
                        .backend
                        .overwrite_allocated(&path, allocation, handle)?;
                    let mut basic = self.mounts[mount].backend.stat(&path)?.basic;
                    basic.attributes = if options.disposition == CreateDisposition::Supersede {
                        (options.attributes & FATX_ATTRIBUTES & !ATTR_DIRECTORY) | ATTR_ARCHIVE
                    } else {
                        basic.attributes
                            | (options.attributes & FATX_ATTRIBUTES & !ATTR_DIRECTORY)
                            | ATTR_ARCHIVE
                    };
                    self.mounts[mount].backend.set_basic_info(&path, basic)?;
                    if options.disposition == CreateDisposition::Supersede {
                        CreateInformation::Superseded
                    } else {
                        CreateInformation::Overwritten
                    }
                } else {
                    CreateInformation::Opened
                }
            } else {
                self.mounts[mount].backend.create_allocated(
                    &path,
                    options.directory == Some(true),
                    options.attributes,
                    allocation,
                    handle,
                )?;
                CreateInformation::Created
            })
        })();
        let information = match opened {
            Ok(information) => information,
            Err(error) => {
                let _ = self.mounts[mount].backend.release_allocation(&path, handle);
                return Err(error);
            }
        };
        self.handles.insert(handle, handle);
        let directory = existing
            .as_ref()
            .map_or(options.directory == Some(true), Metadata::is_directory);
        if !directory {
            self.mounts[mount].backend.opened(&path);
        }
        if !directory && let Some(observe) = self.observer.as_mut() {
            let m = &mut self.mounts[mount];
            observe(&OpenedFile {
                device: m.device.clone(),
                path: path.clone(),
                host: m.backend.host_file(&path),
            });
        }
        self.objects.insert(
            handle,
            OpenFile {
                mount,
                path,
                key: key.clone(),
                options,
                position: 0,
                enumeration: None,
                delete_requested: false,
                directory,
                changed: !directory && information != CreateInformation::Opened,
            },
        );
        Ok(OpenResult {
            handle: Handle(handle),
            information,
        })
    }
    /// NtDuplicateObject: same file object, including position, search cursor and share reservation.
    pub fn duplicate(&mut self, handle: Handle) -> Result<Handle> {
        let object = *self.handles.get(&handle.0).ok_or(Error::InvalidHandle)?;
        let duplicate = self.allocate_handle()?;
        self.handles.insert(duplicate, object);
        Ok(Handle(duplicate))
    }
    pub fn close(&mut self, handle: Handle) -> Result<()> {
        let id = self.handles.remove(&handle.0).ok_or(Error::InvalidHandle)?;
        if self.handles.values().any(|object| *object == id) {
            return Ok(());
        }
        let object = self.objects.remove(&id).ok_or(Error::InvalidHandle)?;
        if !object.directory {
            self.mounts[object.mount].backend.closed(&object.path);
        }
        let key = (object.mount, object.key.clone());
        let write_access = !object.directory
            && object.options.access.0 & (Access::WRITE.0 | Access::APPEND.0) != 0;
        if let Err(error) = self.mounts[object.mount]
            .backend
            .cleanup(&object.path, write_access)
        {
            // The handle is already closed. Preserve close success; explicit flush is
            // the API for callers that need a synchronous durability error.
            eprintln!("vfs: file-object cleanup sync failed: {error}");
        }
        if let Err(error) = self.mounts[object.mount]
            .backend
            .release_allocation(&object.path, id)
        {
            eprintln!("vfs: allocation release failed: {error}");
        }
        if object.options.delete_on_close || object.delete_requested {
            self.pending.insert(key.clone());
            self.cleanup_pending.insert(key.clone());
        }
        if self.pending.contains(&key)
            && !self
                .objects
                .values()
                .any(|f| f.mount == object.mount && f.key == object.key)
        {
            let result = self.mounts[object.mount].backend.remove(&object.path);
            self.pending.remove(&key);
            self.cleanup_pending.remove(&key);
            result?;
            self.report_change(object.mount, &object.path, ChangeKind::Removed);
            return Ok(());
        }
        if object.changed {
            self.report_change(object.mount, &object.path, ChangeKind::Written);
        }
        Ok(())
    }
    pub fn query_information(&mut self, handle: Handle) -> Result<FileInformation> {
        let file = self.object(handle)?;
        let (mount, path, position, access, pending) = (
            file.mount,
            file.path.clone(),
            file.position,
            file.options.access,
            self.pending.contains(&(file.mount, file.key.clone())),
        );
        Ok(FileInformation {
            metadata: self.mounts[mount].backend.stat(&path)?,
            position,
            access,
            delete_pending: pending,
            device_path: if path.is_empty() {
                self.mounts[mount].device.clone()
            } else {
                format!("{}\\{path}", self.mounts[mount].device)
            },
        })
    }
    pub fn query_path(&mut self, path: &str) -> Result<Metadata> {
        let (mount, path) = self.resolve(path)?;
        self.mounts[mount].backend.stat(&path)
    }
    /// For recording a sparse sim store: the exact ranges it can serve.
    pub fn map_ranges(&mut self, name: &str) -> Result<Option<Vec<(u64, u64)>>> {
        let (mount, path) = self.resolve(name)?;
        Ok(self.mounts[mount].backend.map_ranges(&path))
    }

    pub fn query_volume_information(&mut self, handle: Handle) -> Result<VolumeInfo> {
        let mount = self.object(handle)?.mount;
        self.mounts[mount].backend.volume_info()
    }
    pub fn set_position(&mut self, handle: Handle, position: u64) -> Result<()> {
        if position > i64::MAX as u64 {
            return Err(Error::InvalidParameter);
        }
        self.object_mut(handle)?.position = position;
        Ok(())
    }
    pub fn guest_read_bytes(&self) -> GuestReadBytes {
        self.guest_reads
    }

    pub fn note_guest_copy(&mut self, bytes: usize) {
        self.guest_reads.copied += bytes as u64;
    }

    /// Try one NtReadFile before its ordinary bounded-copy loop. A refusal
    /// leaves the file position and the guest's bytes alone. Ordinary reads
    /// report backend errors (including sparse store misses) as usual.
    pub fn try_map_read(
        &mut self,
        handle: Handle,
        offset: Offset,
        length: usize,
        install: impl FnOnce(&ImmutableRead<'_>) -> std::result::Result<usize, &'static str>,
    ) -> Option<usize> {
        if !self.map_guest_reads || length == 0 {
            return None;
        }
        let file = self.object(handle).ok()?;
        if !file.path.as_bytes().get(file.path.len().saturating_sub(4)..)?.eq_ignore_ascii_case(b".map") {
            return None;
        }
        let (mount, path) = (file.mount, file.path.clone());
        let at = match offset {
            Offset::Current => file.position,
            Offset::Absolute(at) if at <= i64::MAX as u64 => at,
            _ => return None,
        };
        let result = if !file.options.access.contains(Access::READ)
            || file.options.access.0 & (Access::WRITE.0 | Access::APPEND.0) != 0 {
            Err("writable handle")
        } else if length < 16 * 4096 {
            Err("short read")
        } else if let Ok(Some(source)) = self.mounts[mount].backend.film_source(&path) {
            match source.immutable_read(at, length) {
                Some(span) => install(&span).map(|mapped| (span.byte_len(), mapped)),
                None => Err("mutable, composed or noncontiguous source"),
            }
        } else {
            Err("no immutable file region")
        };
        match result {
            Ok((len, mapped)) => {
                self.object_mut(handle).expect("open during read").position = at + len as u64;
                if sim_store::record::enabled() {
                    sim_store::record::note(&self.mounts[mount].device, &path, at, len as u64);
                }
                self.guest_reads.mapped += mapped as u64;
                self.note_guest_copy(len - mapped);
                Some(len)
            }
            Err(reason) => {
                if self.read_copy_reasons.insert(reason) {
                    eprintln!("guest mapped read: copying {path} at {at} ({length} bytes): {reason}");
                }
                None
            }
        }
    }

    /// Synchronous I/O: both current and explicit successful reads update the file-object position.
    pub fn read(&mut self, handle: Handle, offset: Offset, buffer: &mut [u8]) -> Result<usize> {
        let file = self.object(handle)?;
        if !file.options.access.contains(Access::READ) {
            return Err(Error::AccessDenied);
        }
        let at = match offset {
            Offset::Current => file.position,
            Offset::Absolute(n) => n,
            Offset::EndOfFile => return Err(Error::InvalidParameter),
        };
        if at > i64::MAX as u64 {
            return Err(Error::InvalidParameter);
        }
        let (mount, path) = (file.mount, file.path.clone());
        let len = self.mounts[mount].backend.read_at(&path, at, buffer)?;
        if len == 0 && !buffer.is_empty() {
            return Err(Error::EndOfFile);
        }
        if sim_store::record::enabled() {
            sim_store::record::note(&self.mounts[mount].device, &path, at, len as u64);
        }
        self.object_mut(handle)?.position = at + len as u64;
        Ok(len)
    }
    pub fn write(&mut self, handle: Handle, offset: Offset, buffer: &[u8]) -> Result<usize> {
        let file = self.object(handle)?;
        if file.options.access.0 & (Access::WRITE.0 | Access::APPEND.0) == 0 {
            return Err(Error::AccessDenied);
        }
        let (mount, path, position) = (file.mount, file.path.clone(), file.position);
        let append_only = !file.options.access.contains(Access::WRITE);
        let at = if append_only || offset == Offset::EndOfFile {
            self.mounts[mount].backend.stat(&path)?.end_of_file
        } else {
            match offset {
                Offset::Current => position,
                Offset::Absolute(n) => n,
                Offset::EndOfFile => unreachable!(),
            }
        };
        if at > i64::MAX as u64 {
            return Err(Error::InvalidParameter);
        }
        let len = self.mounts[mount].backend.write_at(&path, at, buffer)?;
        let object = self.object_mut(handle)?;
        object.position = at + len as u64;
        object.changed |= len > 0;
        Ok(len)
    }
    pub fn set_end_of_file(&mut self, handle: Handle, size: u64) -> Result<()> {
        let file = self.object(handle)?;
        if !file.options.access.contains(Access::WRITE) {
            return Err(Error::AccessDenied);
        }
        let (mount, path) = (file.mount, file.path.clone());
        self.mounts[mount].backend.set_len(&path, size)?;
        self.object_mut(handle)?.changed = true;
        Ok(())
    }
    /// Full logical metadata replacement. Kernel merges zero/-1 NT sentinel fields first.
    pub fn set_basic_information(&mut self, handle: Handle, basic: BasicInfo) -> Result<()> {
        let file = self.object(handle)?;
        if !file.options.access.contains(Access::WRITE_ATTRIBUTES) {
            return Err(Error::AccessDenied);
        }
        let (mount, path) = (file.mount, file.path.clone());
        self.mounts[mount].backend.set_basic_info(&path, basic)
    }
    pub fn set_delete_pending(&mut self, handle: Handle, delete: bool) -> Result<()> {
        let file = self.object(handle)?;
        if !file.options.access.contains(Access::DELETE) {
            return Err(Error::AccessDenied);
        }
        let (mount, path, key) = (file.mount, file.path.clone(), file.key.clone());
        if delete {
            if path.is_empty() {
                return Err(Error::AccessDenied);
            }
            let metadata = self.mounts[mount].backend.stat(&path)?;
            if metadata.basic.attributes & ATTR_READONLY != 0 {
                return Err(Error::AccessDenied);
            }
            if metadata.is_directory()
                && !self.mounts[mount].backend.read_directory(&path)?.is_empty()
            {
                return Err(Error::DirectoryNotEmpty);
            }
        }
        self.object_mut(handle)?.delete_requested = delete;
        if delete {
            self.pending.insert((mount, key));
        } else if !self.cleanup_pending.contains(&(mount, key.clone()))
            && !self
                .objects
                .values()
                .any(|f| f.mount == mount && f.key == key && f.delete_requested)
        {
            self.pending.remove(&(mount, key));
        }
        Ok(())
    }
    pub fn flush(&mut self, handle: Handle) -> Result<()> {
        let file = self.object(handle)?;
        let (mount, path) = (file.mount, file.path.clone());
        self.mounts[mount].backend.flush(&path)
    }
    /// Returns one entry, so kernel buffer overflow never consumes additional entries.
    /// The first expression is captured for the file object; later expressions are ignored.
    /// Restart refreshes the snapshot and rewinds using that captured expression.
    pub fn query_directory(
        &mut self,
        handle: Handle,
        pattern: Option<&str>,
        restart: bool,
    ) -> Result<DirectoryEntry> {
        self.peek_directory(handle, pattern, restart)?;
        self.consume_directory(handle)
    }
    /// Pack first, then call consume_directory only after the entry fits the guest buffer.
    pub fn peek_directory(
        &mut self,
        handle: Handle,
        pattern: Option<&str>,
        restart: bool,
    ) -> Result<DirectoryEntry> {
        let file = self.object(handle)?;
        if !file.options.access.contains(Access::READ) {
            return Err(Error::AccessDenied);
        }
        let first = file.enumeration.is_none();
        if first || restart {
            let expression = file
                .enumeration
                .as_ref()
                .map(|e| e.pattern.as_str())
                .unwrap_or(pattern.filter(|p| !p.is_empty()).unwrap_or("*"));
            if expression.is_empty()
                || !expression.is_ascii()
                || expression
                    .bytes()
                    .any(|b| !(0x20..0x7f).contains(&b) || b"\\/:\"<>|".contains(&b))
            {
                return Err(Error::ObjectNameInvalid);
            }
            let (mount, path, expression) = (file.mount, file.path.clone(), expression.to_owned());
            let mut entries = self.mounts[mount].backend.read_directory(&path)?;
            entries.retain(|e| wildcard_matches(&expression, &e.name));
            entries.sort_by_key(|e| e.name.to_ascii_lowercase());
            self.object_mut(handle)?.enumeration = Some(Enumeration {
                entries,
                next: 0,
                pattern: expression,
            });
        }
        let search = self.object(handle)?.enumeration.as_ref().unwrap();
        search.entries.get(search.next).cloned().ok_or(if first {
            Error::NoSuchFile
        } else {
            Error::NoMoreFiles
        })
    }
    pub fn consume_directory(&mut self, handle: Handle) -> Result<DirectoryEntry> {
        let search = self
            .object_mut(handle)?
            .enumeration
            .as_mut()
            .ok_or(Error::InvalidParameter)?;
        let entry = search
            .entries
            .get(search.next)
            .cloned()
            .ok_or(Error::NoMoreFiles)?;
        search.next += 1;
        Ok(entry)
    }
}

impl Drop for Vfs {
    fn drop(&mut self) {
        // Normal shutdown closes file objects, including delete-on-close requests.
        // Explicit close() is required when the caller needs to observe a cleanup failure.
        let handles: Vec<_> = self.handles.keys().copied().collect();
        for handle in handles {
            let _ = self.close(Handle(handle));
        }
    }
}
