//! LocalDevVPN gate and Wi-Fi detection (siboot-mod): the web page offers to
//! turn the VPN on and waits for it, instead of failing in step 1.

use std::ffi::CStr;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::timeout;

/// Turns LocalDevVPN on and returns to Minis.
pub const ENABLE_URL: &str = "localdevvpn://enable?scheme=minis";

/// Turns LocalDevVPN off and returns to Minis.
pub const DISABLE_URL: &str = "localdevvpn://disable?scheme=minis";

/// True when some network interface whose name passes `named` has an IPv4
/// address. No addresses or subnets are assumed.
fn has_ipv4(named: impl Fn(&str) -> bool) -> bool {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list` with a linked list that is read only
    // here and freed with freeifaddrs before returning.
    unsafe {
        if libc::getifaddrs(&mut list) != 0 {
            return false;
        }
        let mut found = false;
        let mut p = list;
        while !p.is_null() {
            let ifa = &*p;
            if !ifa.ifa_addr.is_null()
                && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET
                && named(&CStr::from_ptr(ifa.ifa_name).to_string_lossy())
            {
                found = true;
                break;
            }
            p = ifa.ifa_next;
        }
        libc::freeifaddrs(list);
        found
    }
}

/// Wi-Fi is `en0` with an IPv4 address; on mobile data it is up but has none.
pub fn wifi_up() -> bool {
    has_ipv4(|name| name == "en0")
}

/// True when LocalDevVPN looks on: a `utun` interface has an IPv4 address (a
/// VPN tunnel), or `addr` answers. A refused connection counts as answered.
/// The port alone is not trusted: on mobile data it can stay silent while the
/// tunnel is up.
pub async fn is_up(addr: SocketAddr) -> bool {
    if has_ipv4(|name| name.starts_with("utun")) {
        return true;
    }
    match timeout(Duration::from_secs(3), TcpStream::connect(addr)).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => e.kind() == ErrorKind::ConnectionRefused,
        Err(_) => false,
    }
}

/// Polls `is_up` for up to `secs` seconds, so coming back from LocalDevVPN
/// doesn't re-ask while the tunnel is still starting.
pub async fn wait_up(addr: SocketAddr, secs: u64) {
    for _ in 0..secs {
        if is_up(addr).await {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
