//! Bonjour discovery for the LAN lab. Discovery yields an address only.
//! The pairing code and pinned TLS identity authenticate the host.

use std::net::{IpAddr, SocketAddr};

use anyhow::{Context, Result, ensure};

pub const PORT: u16 = 29422;

pub fn default_name() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        let output = Command::new("/usr/sbin/scutil")
            .args(["--get", "LocalHostName"])
            .output()
            .context("reading the Mac's LocalHostName")?;
        ensure!(
            output.status.success(),
            "no LocalHostName; give the lab a Bonjour name with --name"
        );
        let name = String::from_utf8(output.stdout)?;
        let name = name.trim();
        ensure!(!name.is_empty(), "empty LocalHostName; specify --name");
        Ok(name.into())
    }
    #[cfg(not(target_os = "macos"))]
    anyhow::bail!("Bonjour discovery is available on macOS");
}

/// Accept a numeric endpoint, DNS hostname with an explicit port, or the
/// exact name of a Bonjour lab instance.
pub fn resolve(host: &str) -> Result<SocketAddr> {
    ensure!(!host.is_empty(), "the lab host name is empty");
    if let Ok(addr) = host.parse::<SocketAddr>() {
        return Ok(addr);
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, PORT));
    }
    if let Some((name, port)) = host.rsplit_once(':')
        && let Ok(port) = port.parse::<u16>()
    {
        return host_address(name, port);
    }
    #[cfg(target_os = "macos")]
    {
        macos::resolve(host).with_context(|| format!("finding Bonjour lab {host:?}"))
    }
    #[cfg(not(target_os = "macos"))]
    anyhow::bail!("use an IP address or hostname:port for the lab host on this platform");
}

fn host_address(name: &str, port: u16) -> Result<SocketAddr> {
    #[cfg(target_os = "macos")]
    {
        macos::host_address(name, port).with_context(|| format!("finding host {name:?}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        use std::net::ToSocketAddrs;

        (name, port)
            .to_socket_addrs()
            .with_context(|| format!("finding host {name:?}"))?
            .next()
            .with_context(|| format!("no address for {name:?}"))
    }
}

#[cfg(target_os = "macos")]
pub use macos::advertise;

#[cfg(not(target_os = "macos"))]
pub struct Advertisement;

#[cfg(not(target_os = "macos"))]
pub fn advertise(_: &str, _: u16) -> Result<Advertisement> {
    anyhow::bail!("Bonjour advertisement is available on macOS");
}

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::io;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
    use std::ptr::{null, null_mut};
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result, bail, ensure};

    const SERVICE: &CStr = c"_h2lab._tcp";
    const DOMAIN: &CStr = c"local.";
    const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(4);
    const IPV4_FALLBACK_WINDOW: Duration = Duration::from_millis(150);

    // The platform ABI is declared in the macOS SDK's <dns_sd.h>.
    type ServiceRef = *mut c_void;
    type RegisterReply = unsafe extern "C" fn(
        ServiceRef,
        u32,
        i32,
        *const c_char,
        *const c_char,
        *const c_char,
        *mut c_void,
    );
    type ResolveReply = unsafe extern "C" fn(
        ServiceRef,
        u32,
        u32,
        i32,
        *const c_char,
        *const c_char,
        u16,
        u16,
        *const u8,
        *mut c_void,
    );
    type AddressReply = unsafe extern "C" fn(
        ServiceRef,
        u32,
        u32,
        i32,
        *const c_char,
        *const libc::sockaddr,
        u32,
        *mut c_void,
    );

    const ADD: u32 = 0x2;
    const NO_AUTO_RENAME: u32 = 0x8;
    const IPV4: u32 = 0x1;
    const IPV6: u32 = 0x2;
    const NAME_CONFLICT: i32 = -65548;
    const NO_SUCH_RECORD: i32 = -65554;

    // macOS exports DNS-SD through libSystem.
    unsafe extern "C" {
        fn DNSServiceRegister(
            service: *mut ServiceRef,
            flags: u32,
            interface: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            host: *const c_char,
            port: u16,
            txt_len: u16,
            txt: *const c_void,
            reply: RegisterReply,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceResolve(
            service: *mut ServiceRef,
            flags: u32,
            interface: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            reply: ResolveReply,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceGetAddrInfo(
            service: *mut ServiceRef,
            flags: u32,
            interface: u32,
            protocol: u32,
            host: *const c_char,
            reply: AddressReply,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceRefSockFD(service: ServiceRef) -> c_int;
        fn DNSServiceProcessResult(service: ServiceRef) -> i32;
        fn DNSServiceRefDeallocate(service: ServiceRef);
    }

    type Replies<T> = RefCell<VecDeque<std::result::Result<T, i32>>>;

    // A DNSServiceRef owns the registration or query until deallocation.
    // The associated callback runs only during DNSServiceProcessResult here.
    struct Observation<T> {
        service: ServiceRef,
        replies: Box<Replies<T>>,
    }

    impl<T> Observation<T> {
        fn start(start: impl FnOnce(*mut ServiceRef, *mut c_void) -> i32) -> Result<Self> {
            let replies = Box::<Replies<T>>::default();
            let context = std::ptr::from_ref(replies.as_ref()).cast_mut().cast();
            let mut service = null_mut();
            check(start(&mut service, context))?;
            Ok(Self { service, replies })
        }

        fn next(&self, deadline: Instant) -> Result<Option<T>> {
            // SAFETY: the reference survives this call and the observation is
            // confined to its owning thread.
            let fd = unsafe { DNSServiceRefSockFD(self.service) };
            ensure!(fd >= 0, "no socket for Bonjour operation");
            let mut pollfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };

            loop {
                match self.replies.borrow_mut().pop_front() {
                    Some(Ok(value)) => return Ok(Some(value)),
                    Some(Err(code)) => check(code)?,
                    None => {}
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Ok(None);
                }
                // SAFETY: the single pollfd is live and notification is only
                // used to read the service reference on this thread.
                let ready =
                    unsafe { libc::poll(&mut pollfd, 1, remaining.as_millis().max(1) as i32) };
                if ready < 0 {
                    let err = io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(err.into());
                }
                if ready > 0 {
                    ensure!(
                        pollfd.revents & libc::POLLIN != 0,
                        "Bonjour connection closed"
                    );
                    // SAFETY: the reference and its callback context are
                    // owned by self and retained throughout processing.
                    check(unsafe { DNSServiceProcessResult(self.service) })?;
                }
            }
        }
    }

    impl<T> Drop for Observation<T> {
        fn drop(&mut self) {
            // SAFETY: this is the only owner and callback processing is
            // synchronous on the owning thread.
            unsafe { DNSServiceRefDeallocate(self.service) };
        }
    }

    fn check(code: i32) -> Result<()> {
        match code {
            0 => Ok(()),
            NAME_CONFLICT => bail!("Bonjour name already in use; specify a different --name"),
            _ => bail!("Bonjour error {code}"),
        }
    }

    unsafe fn report<T>(context: *mut c_void, value: std::result::Result<T, i32>) {
        // SAFETY: every callback's context is its Observation's replies, with
        // the same T as the callback and available for its entire call.
        let replies = unsafe { &*context.cast::<Replies<T>>() };
        replies.borrow_mut().push_back(value);
    }

    pub struct Advertisement {
        _registration: Observation<()>,
    }

    pub fn advertise(name: &str, port: u16) -> Result<Advertisement> {
        let name = CString::new(name).context("Bonjour name contains NUL")?;
        ensure!(
            (1..=63).contains(&name.as_bytes().len()),
            "the Bonjour name must be between 1 and 63 bytes"
        );
        ensure!(port != 0, "the Bonjour registration needs a TCP port");
        // SAFETY: strings are valid for the call and the context is retained
        // by Observation. Bonjour copies the registration data.
        let registration = Observation::<()>::start(|service, context| unsafe {
            DNSServiceRegister(
                service,
                NO_AUTO_RENAME,
                0,
                name.as_ptr(),
                SERVICE.as_ptr(),
                DOMAIN.as_ptr(),
                null(),
                port.to_be(),
                0,
                null(),
                registered,
                context,
            )
        })?;
        registration
            .next(Instant::now() + DISCOVERY_TIMEOUT)?
            .context("timed out registering the Bonjour lab")?;
        Ok(Advertisement {
            _registration: registration,
        })
    }

    unsafe extern "C" fn registered(
        _: ServiceRef,
        _: u32,
        code: i32,
        _: *const c_char,
        _: *const c_char,
        _: *const c_char,
        context: *mut c_void,
    ) {
        // SAFETY: advertise supplies the context for a unit result.
        unsafe { report(context, if code == 0 { Ok(()) } else { Err(code) }) };
    }

    struct Resolved {
        host: CString,
        port: u16,
        interface: u32,
    }

    pub(super) fn resolve(name: &str) -> Result<SocketAddr> {
        let name = CString::new(name).context("Bonjour name contains NUL")?;
        let deadline = Instant::now() + DISCOVERY_TIMEOUT;
        // SAFETY: strings survive the call and the callback context survives
        // all processing through Observation.
        let query = Observation::<Resolved>::start(|service, context| unsafe {
            DNSServiceResolve(
                service,
                0,
                0,
                name.as_ptr(),
                SERVICE.as_ptr(),
                DOMAIN.as_ptr(),
                resolved,
                context,
            )
        })?;
        let Resolved {
            host,
            port,
            interface,
        } = query
            .next(deadline)?
            .context("timed out resolving the lab; use its IP address and port if needed")?;
        lookup(&host, port, interface, deadline)
    }

    unsafe extern "C" fn resolved(
        _: ServiceRef,
        _: u32,
        interface: u32,
        code: i32,
        _: *const c_char,
        host: *const c_char,
        port: u16,
        _: u16,
        _: *const u8,
        context: *mut c_void,
    ) {
        // SAFETY: on success Bonjour supplies a valid host string through
        // this callback. Copy it before returning.
        let result = if code == 0 {
            Ok(Resolved {
                host: unsafe { CStr::from_ptr(host) }.to_owned(),
                port: u16::from_be(port),
                interface,
            })
        } else {
            Err(code)
        };
        // SAFETY: resolve supplies the context for a Resolved result.
        unsafe { report(context, result) };
    }

    pub(super) fn host_address(name: &str, port: u16) -> Result<SocketAddr> {
        let host = CString::new(name).context("hostname contains NUL")?;
        lookup(&host, port, 0, Instant::now() + DISCOVERY_TIMEOUT)
    }

    fn lookup(host: &CStr, port: u16, interface: u32, deadline: Instant) -> Result<SocketAddr> {
        // SAFETY: host survives the call and the callback context survives
        // all processing through Observation.
        let query = Observation::<SocketAddr>::start(|service, context| unsafe {
            DNSServiceGetAddrInfo(
                service,
                0,
                interface,
                IPV4 | IPV6,
                host.as_ptr(),
                address,
                context,
            )
        })?;
        let mut addr = query
            .next(deadline)?
            .context("timed out finding the lab address")?;
        if addr.is_ipv6() {
            // The default lab listener binds IPv4. Give its A record time
            // to arrive when the AAAA record is delivered first.
            let window = deadline.min(Instant::now() + IPV4_FALLBACK_WINDOW);
            while let Some(other) = query.next(window)? {
                if other.is_ipv4() {
                    addr = other;
                    break;
                }
            }
        }
        addr.set_port(port);
        Ok(addr)
    }

    unsafe extern "C" fn address(
        _: ServiceRef,
        flags: u32,
        interface: u32,
        code: i32,
        _: *const c_char,
        address: *const libc::sockaddr,
        _: u32,
        context: *mut c_void,
    ) {
        if code == NO_SUCH_RECORD {
            return; // The other address family can still answer.
        }
        if code != 0 {
            // SAFETY: lookup supplies the context for a SocketAddr result.
            unsafe { report::<SocketAddr>(context, Err(code)) };
            return;
        }
        if flags & ADD == 0 || address.is_null() {
            return;
        }

        // SAFETY: Bonjour provides a sockaddr of the reported address family
        // for the duration of this callback.
        let addr: SocketAddr = match i32::from(unsafe { (*address).sa_family }) {
            libc::AF_INET => {
                let addr = unsafe { &*address.cast::<libc::sockaddr_in>() };
                (Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes()), 0).into()
            }
            libc::AF_INET6 => {
                let addr = unsafe { &*address.cast::<libc::sockaddr_in6>() };
                let ip = Ipv6Addr::from(addr.sin6_addr.s6_addr);
                let scope = if ip.is_unicast_link_local() && addr.sin6_scope_id == 0 {
                    interface
                } else {
                    addr.sin6_scope_id
                };
                SocketAddrV6::new(ip, 0, 0, scope).into()
            }
            _ => return,
        };
        // SAFETY: lookup supplies the context for a SocketAddr result.
        unsafe { report(context, Ok(addr)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_addresses_bypass_bonjour() {
        for (host, expected) in [
            ("127.0.0.1", "127.0.0.1:29422"),
            ("127.0.0.1:12000", "127.0.0.1:12000"),
            ("::1", "[::1]:29422"),
            ("[::1]:12000", "[::1]:12000"),
        ] {
            assert_eq!(
                resolve(host).unwrap(),
                expected.parse::<SocketAddr>().unwrap()
            );
        }
    }
}
