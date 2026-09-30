//! Adresy IPv4 dzialajacych interfejsow - do multicastu na kazdym z nich.
//!
//! `join_multicast_v4(.., 0.0.0.0)` i `send_to` bez `IP_MULTICAST_IF` ida
//! przez interfejs z trasa domyslna. Z wlaczonym VPN-em to tunel: beacony
//! nie docieraja do Wi-Fi, a cudze z Wi-Fi nie docieraja do nas (30 IX 2026,
//! dwa komputery w jednej sieci sie nie widzialy).

use std::net::Ipv4Addr;

/// Adresy interfejsow w stanie "up", bez petli zwrotnej i bez link-local
/// (169.254.x.x - interfejs bez DHCP, np. rozlaczony Tailscale).
pub fn local_ipv4() -> Vec<Ipv4Addr> {
    let mut out: Vec<Ipv4Addr> = raw().into_iter().filter(|a| usable(*a)).collect();
    out.sort();
    out.dedup();
    out
}

fn usable(a: Ipv4Addr) -> bool {
    !a.is_loopback() && !a.is_link_local() && !a.is_unspecified() && !a.is_multicast()
}

#[cfg(windows)]
fn raw() -> Vec<Ipv4Addr> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;

    const AF_INET: u32 = 2;
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;

    // Bufor wyrownany do 8 (struktury maja wskazniki); zwykle 16 KB wystarcza.
    let mut buf: Vec<u64> = vec![0; 2048];
    let mut out = Vec::new();
    for _ in 0..3 {
        let mut size = (buf.len() * 8) as u32;
        let head = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
        let r = unsafe { GetAdaptersAddresses(AF_INET, flags, None, Some(head), &mut size) };
        if r == ERROR_BUFFER_OVERFLOW {
            buf = vec![0; (size as usize).div_ceil(8)];
            continue;
        }
        if r != 0 {
            return out;
        }
        let mut a = head as *const IP_ADAPTER_ADDRESSES_LH;
        while !a.is_null() {
            let ad = unsafe { &*a };
            if ad.OperStatus == IfOperStatusUp && ad.IfType != IF_TYPE_SOFTWARE_LOOPBACK {
                let mut u = ad.FirstUnicastAddress;
                while !u.is_null() {
                    let ua = unsafe { &*u };
                    let sa = ua.Address.lpSockaddr as *const u8;
                    // SOCKADDR_IN: rodzina (u16), port (u16), adres (4 bajty).
                    if !sa.is_null() && ua.Address.iSockaddrLength >= 8 {
                        let fam = unsafe { u16::from_ne_bytes([*sa, *sa.add(1)]) };
                        if u32::from(fam) == AF_INET {
                            let b = unsafe { [*sa.add(4), *sa.add(5), *sa.add(6), *sa.add(7)] };
                            out.push(Ipv4Addr::from(b));
                        }
                    }
                    u = ua.Next;
                }
            }
            a = ad.Next;
        }
        return out;
    }
    out
}

#[cfg(not(windows))]
fn raw() -> Vec<Ipv4Addr> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bez_petli_i_link_local() {
        assert!(!usable(Ipv4Addr::LOCALHOST));
        assert!(!usable(Ipv4Addr::new(169, 254, 83, 107)));
        assert!(usable(Ipv4Addr::new(192, 168, 1, 20)));
        assert!(usable(Ipv4Addr::new(100, 64, 0, 7)));
    }

    /// Na maszynie z siecia lista nie jest pusta i nie ma w niej petli zwrotnej.
    #[test]
    #[cfg(windows)]
    fn lista_interfejsow() {
        let l = local_ipv4();
        eprintln!("interfejsy: {l:?}");
        assert!(l.iter().all(|a| usable(*a)));
    }
}
