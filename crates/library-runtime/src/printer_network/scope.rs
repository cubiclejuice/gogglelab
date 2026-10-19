use std::collections::BTreeSet;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};

pub const MAX_SCAN_ADDRESSES: usize = 1_022;
const MAX_SCAN_SCOPE_BYTES: usize = 4_096;
const MAX_SCAN_SCOPE_TERMS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanScope {
    addresses: Vec<Ipv4Addr>,
    display: String,
}

impl ScanScope {
    pub fn addresses(&self) -> &[Ipv4Addr] {
        &self.addresses
    }
}

impl fmt::Display for ScanScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.display)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanScopeError {
    Empty,
    InputTooLong,
    TooManyTerms,
    InvalidSyntax,
    InvalidPrefix,
    ReversedRange,
    AddressOutsidePrivateNetwork,
    TooManyAddresses,
}

impl fmt::Display for ScanScopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "Enter an IPv4 CIDR or address range",
            Self::InputTooLong => "Scan scope text exceeds its size limit",
            Self::TooManyTerms => "Scan scope contains too many entries",
            Self::InvalidSyntax => "Scan scope must use IPv4 CIDR or start-end notation",
            Self::InvalidPrefix => "IPv4 CIDR prefix must be between 0 and 32",
            Self::ReversedRange => "Scan range start must not exceed its end",
            Self::AddressOutsidePrivateNetwork => {
                "Scan scope may include only private unicast IPv4 addresses"
            }
            Self::TooManyAddresses => "Scan scope exceeds the 1,022-address limit",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ScanScopeError {}

/// Parses comma-separated IPv4 CIDRs and inclusive start-end ranges.
///
/// CIDRs with prefixes through /30 omit their network and directed-broadcast
/// addresses. /31 and /32 retain their addresses. Ranges are inclusive. The
/// returned addresses are sorted and de-duplicated, and the display string
/// represents exactly that normalized address set.
pub fn parse_scan_scope(input: &str) -> Result<ScanScope, ScanScopeError> {
    if input.len() > MAX_SCAN_SCOPE_BYTES {
        return Err(ScanScopeError::InputTooLong);
    }
    let input = input.trim();
    if input.is_empty() {
        return Err(ScanScopeError::Empty);
    }

    let terms = input.split(',').collect::<Vec<_>>();
    if terms.len() > MAX_SCAN_SCOPE_TERMS {
        return Err(ScanScopeError::TooManyTerms);
    }

    let mut addresses = BTreeSet::new();
    for term in terms {
        let term = term.trim();
        if term.is_empty() {
            return Err(ScanScopeError::InvalidSyntax);
        }
        if term.contains('/') {
            add_cidr(term, &mut addresses)?;
        } else if term.contains('-') || term.contains('–') {
            add_range(term, &mut addresses)?;
        } else {
            add_single_host(term, &mut addresses)?;
        }
    }

    if addresses.is_empty() {
        return Err(ScanScopeError::Empty);
    }

    let addresses = addresses.into_iter().collect::<Vec<_>>();
    let display = display_scope(&addresses);
    Ok(ScanScope { addresses, display })
}

fn add_cidr(term: &str, addresses: &mut BTreeSet<Ipv4Addr>) -> Result<(), ScanScopeError> {
    let (address_text, prefix_text) = term.split_once('/').ok_or(ScanScopeError::InvalidSyntax)?;
    if prefix_text.contains('/') {
        return Err(ScanScopeError::InvalidSyntax);
    }
    let address = address_text
        .trim()
        .parse::<Ipv4Addr>()
        .map_err(|_| ScanScopeError::InvalidSyntax)?;
    let prefix_text = prefix_text.trim();
    if prefix_text.is_empty() || !prefix_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ScanScopeError::InvalidPrefix);
    }
    let prefix = prefix_text
        .parse::<u8>()
        .map_err(|_| ScanScopeError::InvalidPrefix)?;
    if prefix > 32 {
        return Err(ScanScopeError::InvalidPrefix);
    }

    let address = u32::from(address);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix))
    };
    let network = address & mask;
    let broadcast = network | !mask;
    let (first, last) = if prefix <= 30 {
        (network + 1, broadcast - 1)
    } else {
        (network, broadcast)
    };
    let count = u64::from(last) - u64::from(first) + 1;
    if count > MAX_SCAN_ADDRESSES as u64 {
        return Err(ScanScopeError::TooManyAddresses);
    }

    for value in first..=last {
        insert_address(Ipv4Addr::from(value), addresses)?;
    }
    Ok(())
}

fn add_single_host(term: &str, addresses: &mut BTreeSet<Ipv4Addr>) -> Result<(), ScanScopeError> {
    let address = term
        .trim()
        .parse::<Ipv4Addr>()
        .map_err(|_| ScanScopeError::InvalidSyntax)?;
    insert_address(address, addresses)
}

fn add_range(term: &str, addresses: &mut BTreeSet<Ipv4Addr>) -> Result<(), ScanScopeError> {
    let separator = match (term.contains('-'), term.contains('–')) {
        (true, false) => '-',
        (false, true) => '–',
        _ => return Err(ScanScopeError::InvalidSyntax),
    };
    let mut ends = term.split(separator);
    let start = ends
        .next()
        .unwrap_or_default()
        .trim()
        .parse::<Ipv4Addr>()
        .map_err(|_| ScanScopeError::InvalidSyntax)?;
    let end = ends
        .next()
        .unwrap_or_default()
        .trim()
        .parse::<Ipv4Addr>()
        .map_err(|_| ScanScopeError::InvalidSyntax)?;
    if ends.next().is_some() {
        return Err(ScanScopeError::InvalidSyntax);
    }

    let start = u32::from(start);
    let end = u32::from(end);
    if start > end {
        return Err(ScanScopeError::ReversedRange);
    }
    let count = u64::from(end) - u64::from(start) + 1;
    if count > MAX_SCAN_ADDRESSES as u64 {
        return Err(ScanScopeError::TooManyAddresses);
    }

    for value in start..=end {
        insert_address(Ipv4Addr::from(value), addresses)?;
    }
    Ok(())
}

fn insert_address(
    address: Ipv4Addr,
    addresses: &mut BTreeSet<Ipv4Addr>,
) -> Result<(), ScanScopeError> {
    if !is_private_unicast(address) {
        return Err(ScanScopeError::AddressOutsidePrivateNetwork);
    }
    addresses.insert(address);
    if addresses.len() > MAX_SCAN_ADDRESSES {
        return Err(ScanScopeError::TooManyAddresses);
    }
    Ok(())
}

pub(crate) fn is_private_unicast(address: Ipv4Addr) -> bool {
    address.is_private()
        && !address.is_loopback()
        && !address.is_link_local()
        && !address.is_multicast()
        && !address.is_unspecified()
        && address != Ipv4Addr::BROADCAST
}

/// Resolves the source IPv4 address selected by the OS route without sending a packet.
pub(crate) fn route_interface(socket: &UdpSocket, target: Ipv4Addr) -> Option<Ipv4Addr> {
    socket
        .connect(SocketAddr::V4(SocketAddrV4::new(target, 9)))
        .ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(interface) if is_private_unicast(interface) => Some(interface),
        _ => None,
    }
}

fn display_scope(addresses: &[Ipv4Addr]) -> String {
    let mut ranges = Vec::new();
    let mut iter = addresses.iter().copied();
    let Some(mut start) = iter.next() else {
        return String::new();
    };
    let mut end = start;

    for address in iter {
        if u32::from(address) == u32::from(end) + 1 {
            end = address;
        } else {
            ranges.push(format_range(start, end));
            start = address;
            end = address;
        }
    }
    ranges.push(format_range(start, end));

    let count = addresses.len();
    let noun = if count == 1 { "address" } else { "addresses" };
    format!("{} ({count} {noun})", ranges.join(", "))
}

fn format_range(start: Ipv4Addr, end: Ipv4Addr) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_scan_scope, ScanScopeError};
    use std::net::Ipv4Addr;

    #[test]
    fn cidr_scope_excludes_network_and_broadcast_and_displays_canonically() {
        let scope = parse_scan_scope("192.168.1.0/24").unwrap();

        assert_eq!(scope.addresses().len(), 254);
        assert_eq!(
            scope.addresses().first(),
            Some(&Ipv4Addr::new(192, 168, 1, 1))
        );
        assert_eq!(
            scope.addresses().last(),
            Some(&Ipv4Addr::new(192, 168, 1, 254))
        );
        assert_eq!(
            scope.to_string(),
            "192.168.1.1-192.168.1.254 (254 addresses)"
        );
    }

    #[test]
    fn accepts_the_1022_usable_hosts_of_a_slash_22() {
        let scope = parse_scan_scope("10.8.0.0/22").unwrap();

        assert_eq!(scope.addresses().len(), 1022);
        assert_eq!(scope.addresses().first(), Some(&Ipv4Addr::new(10, 8, 0, 1)));
        assert_eq!(
            scope.addresses().last(),
            Some(&Ipv4Addr::new(10, 8, 3, 254))
        );
    }

    #[test]
    fn supports_private_cidr_prefixes_from_slash_22_through_slash_30() {
        for prefix in 22..=30 {
            let scope = parse_scan_scope(&format!("10.16.0.0/{prefix}")).unwrap();
            let expected = (1_usize << (32 - prefix)) - 2;
            assert_eq!(scope.addresses().len(), expected, "prefix /{prefix}");
        }
    }

    #[test]
    fn ranges_are_inclusive_and_accept_an_en_dash_separator() {
        let scope = parse_scan_scope("192.168.1.10 – 192.168.1.12").unwrap();

        assert_eq!(
            scope.addresses(),
            &[
                Ipv4Addr::new(192, 168, 1, 10),
                Ipv4Addr::new(192, 168, 1, 11),
                Ipv4Addr::new(192, 168, 1, 12),
            ]
        );
        assert_eq!(scope.to_string(), "192.168.1.10-192.168.1.12 (3 addresses)");
    }

    #[test]
    fn accepts_a_start_end_range_at_the_address_cap() {
        let scope = parse_scan_scope("10.0.0.1-10.0.3.254").unwrap();

        assert_eq!(scope.addresses().len(), 1022);
        assert_eq!(scope.addresses().first(), Some(&Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(
            scope.addresses().last(),
            Some(&Ipv4Addr::new(10, 0, 3, 254))
        );
    }

    #[test]
    fn accepts_explicit_single_hosts_as_addresses_cidrs_or_ranges() {
        for input in ["192.168.2.7", "192.168.2.7/32", "192.168.2.7-192.168.2.7"] {
            let scope = parse_scan_scope(input).unwrap();
            assert_eq!(scope.addresses(), &[Ipv4Addr::new(192, 168, 2, 7)]);
            assert_eq!(scope.to_string(), "192.168.2.7 (1 address)");
        }
    }

    #[test]
    fn multiple_scopes_are_sorted_deduplicated_and_compacted_for_display() {
        let scope =
            parse_scan_scope("192.168.1.8-192.168.1.10, 192.168.1.9/32, 192.168.1.5/32").unwrap();

        assert_eq!(
            scope.addresses(),
            &[
                Ipv4Addr::new(192, 168, 1, 5),
                Ipv4Addr::new(192, 168, 1, 8),
                Ipv4Addr::new(192, 168, 1, 9),
                Ipv4Addr::new(192, 168, 1, 10),
            ]
        );
        assert_eq!(
            scope.to_string(),
            "192.168.1.5, 192.168.1.8-192.168.1.10 (4 addresses)"
        );
    }

    #[test]
    fn rejects_any_scope_containing_a_non_private_or_non_unicast_address() {
        for input in [
            "8.8.8.8/32",
            "127.0.0.1/32",
            "169.254.1.2/32",
            "224.0.0.1/32",
            "255.255.255.255/32",
            "0.0.0.0/32",
            "192.168.1.1-192.168.1.2, 8.8.8.8/32",
        ] {
            assert!(parse_scan_scope(input).is_err(), "accepted {input}");
        }
    }

    #[test]
    fn rejects_malformed_reversed_and_ipv6_scopes() {
        for input in [
            "",
            "192.168.1.0/",
            "192.168.1.0/+24",
            "192.168.1.0/33",
            "192.168.1.0/24/1",
            "192.168.1.1-192.168.1.0",
            "192.168.1.1-",
            "-192.168.1.2",
            "192.168.1.1-192.168.1.2-192.168.1.3",
            "192.168.1.0/24,",
            "2001:db8::/64",
        ] {
            assert!(parse_scan_scope(input).is_err(), "accepted {input:?}");
        }
    }

    #[test]
    fn rejects_cidr_and_range_scopes_over_the_address_cap() {
        for input in [
            "10.0.0.0/21",
            "10.0.0.1-10.0.3.255",
            "10.0.0.0/22,10.0.4.0/22",
        ] {
            assert!(parse_scan_scope(input).is_err(), "accepted {input}");
        }
    }

    #[test]
    fn detects_full_ipv4_range_overflow_before_address_validation() {
        assert_eq!(
            parse_scan_scope("0.0.0.0-255.255.255.255").unwrap_err(),
            ScanScopeError::TooManyAddresses
        );
    }

    #[test]
    fn bounds_scope_text_and_term_count() {
        assert_eq!(
            parse_scan_scope(&"x".repeat(4097)).unwrap_err(),
            ScanScopeError::InputTooLong
        );

        let repeated = (0..65)
            .map(|_| "192.168.1.1/32")
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            parse_scan_scope(&repeated).unwrap_err(),
            ScanScopeError::TooManyTerms
        );
    }

    #[test]
    fn supports_private_ipv4_cidr_boundaries_without_scanning_network_or_broadcast() {
        let scope = parse_scan_scope("192.168.1.0/30").unwrap();
        assert_eq!(
            scope.addresses(),
            &[Ipv4Addr::new(192, 168, 1, 1), Ipv4Addr::new(192, 168, 1, 2)]
        );

        let point_to_point = parse_scan_scope("172.16.2.0/31").unwrap();
        assert_eq!(
            point_to_point.addresses(),
            &[Ipv4Addr::new(172, 16, 2, 0), Ipv4Addr::new(172, 16, 2, 1)]
        );
    }
}
