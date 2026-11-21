//! The guest-memory interface the device is written against.
//!
//! The device never sees a host pointer into guest memory: everything goes
//! through [`GuestMemory`], so the same code runs over the runtime's `Guest`
//! (via a small adapter in the integration layer, see `abi.rs`) and over
//! [`SparseMemory`] in tests.

use std::collections::BTreeMap;

/// A guest access that hit unmapped memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("guest access of {len} bytes at {addr:#010x} is outside mapped memory")]
pub struct GuestError {
    /// First byte of the access.
    pub addr: u32,
    /// Length of the access.
    pub len: u32,
}

/// Byte-addressed guest memory (little endian, guest virtual addresses).
///
/// Writes made through this trait are the device's own (headers, state
/// arrays, the push-buffer put pointer). They do not need to go through the
/// write tracker: the device knows what it wrote. Writes made by *guest code*
/// do, see [`crate::dirty`].
pub trait GuestMemory {
    /// Copies `out.len()` bytes starting at `addr`.
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), GuestError>;
    /// Stores `data` at `addr`.
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), GuestError>;

    /// [`write`](Self::write) that may fail quietly: `false` means nothing
    /// was written, and the failure must not be recorded as the fault the
    /// runtime later reports (an adapter that keeps the first fault would
    /// otherwise keep this speculative one). Callers fall back to narrower
    /// writes that fault for real. The default is `write(..).is_ok()`.
    fn try_write(&mut self, addr: u32, data: &[u8]) -> bool {
        self.write(addr, data).is_ok()
    }

    /// Reads a little-endian `u32`.
    fn read_u32(&self, addr: u32) -> Result<u32, GuestError> {
        let mut b = [0; 4];
        self.read(addr, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    /// Writes a little-endian `u32`.
    fn write_u32(&mut self, addr: u32, v: u32) -> Result<(), GuestError> {
        self.write(addr, &v.to_le_bytes())
    }
    /// Reads an `f32` (bit pattern of a `u32`).
    fn read_f32(&self, addr: u32) -> Result<f32, GuestError> {
        self.read_u32(addr).map(f32::from_bits)
    }
    /// Reads `len` bytes into a new vector. Ranges that wrap past the top
    /// of the address space or exceed [`MAX_READ`] fail before anything is
    /// allocated (guest-controlled lengths must not exhaust host memory).
    fn read_vec(&self, addr: u32, len: usize) -> Result<Vec<u8>, GuestError> {
        check_range(addr, len)?;
        let mut v = vec![0; len];
        self.read(addr, &mut v)?;
        Ok(v)
    }
    /// Appends the `len` bytes at `addr` to `out` (same limits as
    /// [`read_vec`](Self::read_vec)); `out` is unchanged on an error. The
    /// default reads into zeroed space; an implementation that can copy
    /// straight from its memory skips the zeroing.
    fn read_append(&self, addr: u32, len: usize, out: &mut Vec<u8>) -> Result<(), GuestError> {
        check_range(addr, len)?;
        let start = out.len();
        out.resize(start + len, 0);
        let r = self.read(addr, &mut out[start..]);
        if r.is_err() {
            out.truncate(start);
        }
        r
    }
    /// Reads `n` consecutive `u32`s (same limits as [`read_vec`](Self::read_vec)).
    fn read_words(&self, addr: u32, n: usize) -> Result<Vec<u32>, GuestError> {
        let len = n.checked_mul(4).ok_or(GuestError {
            addr,
            len: u32::MAX,
        })?;
        check_range(addr, len)?;
        let mut words = vec![0u32; n];
        self.read(addr, bytemuck::cast_slice_mut(&mut words))?;
        // The guest's words are little endian; a no-op on a little-endian host.
        for w in &mut words {
            *w = u32::from_le(*w);
        }
        Ok(words)
    }
}

/// Reads `N` consecutive `u32`s into an array without allocating (the small
/// fixed-size header reads the device makes per draw). Same limits and
/// errors as [`GuestMemory::read_words`]; `N` is at most 64.
pub fn read_array<const N: usize>(
    mem: &dyn GuestMemory,
    addr: u32,
) -> Result<[u32; N], GuestError> {
    const { assert!(N <= 64) };
    check_range(addr, 4 * N)?;
    let mut bytes = [0u8; 256];
    mem.read(addr, &mut bytes[..4 * N])?;
    Ok(std::array::from_fn(|i| {
        u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().expect("4 bytes"))
    }))
}

/// Largest single read the device makes from guest memory: the Xbox has
/// 64 MiB of RAM (128 MiB on debug kits), so nothing legitimate is bigger.
pub const MAX_READ: usize = 128 << 20;

/// Fails if `[addr, addr + len)` wraps or is larger than [`MAX_READ`].
pub fn check_range(addr: u32, len: usize) -> Result<(), GuestError> {
    let err = GuestError {
        addr,
        len: u32::try_from(len).unwrap_or(u32::MAX),
    };
    if len > MAX_READ || addr as u64 + len as u64 > 1 << 32 {
        return Err(err);
    }
    Ok(())
}

const PAGE: u32 = 4096;

/// Page-granular sparse guest memory for tests and tools. Only pages that
/// were [`map`](SparseMemory::map)ped are accessible; everything else faults.
#[derive(Default, Clone)]
pub struct SparseMemory {
    pages: BTreeMap<u32, Box<[u8; PAGE as usize]>>,
}

impl SparseMemory {
    /// Empty memory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Maps (zero-filled) every page overlapping `[addr, addr + len)`.
    pub fn map(&mut self, addr: u32, len: u32) {
        if len == 0 {
            return;
        }
        let first = addr / PAGE;
        let last = (addr as u64 + len as u64 - 1) / PAGE as u64;
        for p in first as u64..=last {
            self.pages
                .entry(p as u32)
                .or_insert_with(|| Box::new([0; PAGE as usize]));
        }
    }

    fn walk(
        &self,
        addr: u32,
        len: usize,
        mut f: impl FnMut(&[u8; PAGE as usize], usize, usize, usize),
    ) -> Result<(), GuestError> {
        let err = GuestError {
            addr,
            len: len as u32,
        };
        if addr as u64 + len as u64 > 1 << 32 {
            return Err(err);
        }
        let mut done = 0usize;
        while done < len {
            let a = addr + done as u32;
            let page = self.pages.get(&(a / PAGE)).ok_or(err)?;
            let off = (a % PAGE) as usize;
            let n = (PAGE as usize - off).min(len - done);
            f(page, off, done, n);
            done += n;
        }
        Ok(())
    }
}

impl GuestMemory for SparseMemory {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), GuestError> {
        self.walk(addr, out.len(), |page, off, done, n| {
            out[done..done + n].copy_from_slice(&page[off..off + n])
        })
    }

    fn read_append(&self, addr: u32, len: usize, out: &mut Vec<u8>) -> Result<(), GuestError> {
        check_range(addr, len)?;
        let start = out.len();
        out.reserve(len);
        let r = self.walk(addr, len, |page, off, _, n| {
            out.extend_from_slice(&page[off..off + n])
        });
        if r.is_err() {
            out.truncate(start);
        }
        r
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), GuestError> {
        // Validate the whole range first so a fault leaves memory untouched.
        self.walk(addr, data.len(), |_, _, _, _| {})?;
        let mut done = 0usize;
        while done < data.len() {
            let a = addr + done as u32;
            let page = self.pages.get_mut(&(a / PAGE)).expect("validated");
            let off = (a % PAGE) as usize;
            let n = (PAGE as usize - off).min(data.len() - done);
            page[off..off + n].copy_from_slice(&data[done..done + n]);
            done += n;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_page_access_and_faults() {
        let mut m = SparseMemory::new();
        m.map(0x1ffe, 4);
        m.write_u32(0x1ffe, 0xdead_beef).unwrap();
        assert_eq!(m.read_u32(0x1ffe).unwrap(), 0xdead_beef);
        assert!(m.read_u32(0x3000).is_err());
        // A write straddling into an unmapped page changes nothing.
        assert!(m.write(0x2ffe, &[1, 2, 3, 4]).is_err());
        assert_eq!(m.read_vec(0x2ffc, 4).unwrap(), vec![0; 4]);
    }

    #[test]
    fn read_array_matches_read_words_and_faults_alike() {
        let mut m = SparseMemory::new();
        m.map(0x1000, 0x1000);
        for i in 0..8 {
            m.write_u32(0x1ff0 - 16 + 4 * i, 0x100 + i).unwrap();
        }
        assert_eq!(
            read_array::<5>(&m, 0x1fec).unwrap().to_vec(),
            m.read_words(0x1fec, 5).unwrap()
        );
        assert_eq!(
            read_array::<5>(&m, 0x1ff8).unwrap_err(),
            m.read_words(0x1ff8, 5).unwrap_err()
        );
        assert_eq!(
            read_array::<5>(&m, 0xffff_fff8).unwrap_err(),
            m.read_words(0xffff_fff8, 5).unwrap_err()
        );
    }

    #[test]
    fn huge_or_wrapping_reads_fail_without_allocating() {
        let m = SparseMemory::new();
        assert!(m.read_vec(0x1000, usize::MAX).is_err());
        assert!(m.read_vec(0xffff_fffc, 8).is_err());
        assert!(m.read_words(0, usize::MAX / 2).is_err());
        assert!(m.read_words(0, 1 << 30).is_err());
    }
}
