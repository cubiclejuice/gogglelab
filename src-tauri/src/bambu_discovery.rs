use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

use crate::printer_network::ScanScope;

const MAX_SSDP_PACKET_BYTES: usize = 4 * 1024;
const MAX_SSDP_RESPONSES: usize = 64;
const MAX_SSDP_INGRESS_DATAGRAMS: usize = 256;
const MAX_SSDP_TOTAL_BYTES: usize = 64 * 1024;
const MAX_SCOPE_INTERFACES: usize = 64;
const BAMBU_SEARCH_TARGET: &str = "urn:bambulab-com:device:3dprinter:1";
const BAMBU_SSDP_ADDRESS: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const BAMBU_SSDP_PORT: u16 = 2021;
const BAMBU_M_SEARCH: &[u8] = b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:2021\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: urn:bambulab-com:device:3dprinter:1\r\n\r\n";

#[derive(Debug, Clone, PartialEq, Eq)]
struct BambuSsdpReply {
    source: Ipv4Addr,
}

#[derive(Default)]
struct SsdpIngressBudget {
    datagrams: usize,
    bytes: usize,
}

impl SsdpIngressBudget {
    fn has_capacity(&self) -> bool {
        self.datagrams < MAX_SSDP_INGRESS_DATAGRAMS && self.bytes < MAX_SSDP_TOTAL_BYTES
    }

    fn record(&mut self, bytes: usize) -> bool {
        self.datagrams = self.datagrams.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
        self.datagrams <= MAX_SSDP_INGRESS_DATAGRAMS && self.bytes <= MAX_SSDP_TOTAL_BYTES
    }
}

pub(crate) fn search(
    scope: &ScanScope,
    deadline: Instant,
    request_timeout: Duration,
    cancelled: &AtomicBool,
) -> Vec<(Ipv4Addr, Vec<u8>)> {
    if crate::visual_qa::require_printer_network().is_err() {
        return Vec::new();
    }
    let interfaces = routed_interfaces(scope, deadline, cancelled);
    let mut replies = Vec::new();
    let mut ingress_budget = SsdpIngressBudget::default();
    for interface in interfaces {
        if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
            break;
        }
        let timeout = request_timeout.min(deadline.saturating_duration_since(Instant::now()));
        if timeout.is_zero() {
            break;
        }
        search_interface(
            interface,
            scope,
            timeout,
            deadline,
            cancelled,
            &mut ingress_budget,
            &mut replies,
        );
        if replies.len() >= MAX_SSDP_RESPONSES || !ingress_budget.has_capacity() {
            break;
        }
    }
    replies
}

pub(crate) fn validated_candidates(
    scope: &ScanScope,
    responses: impl IntoIterator<Item = (Ipv4Addr, Vec<u8>)>,
) -> Vec<Ipv4Addr> {
    let mut candidates = BTreeSet::new();
    let mut ingress_budget = SsdpIngressBudget::default();
    for (source, packet) in responses {
        if !ingress_budget.record(packet.len()) {
            break;
        }
        if scope.addresses().binary_search(&source).is_err() {
            continue;
        }
        if parse_search_response(source, &packet).is_some() {
            candidates.insert(source);
        }
        if candidates.len() >= MAX_SSDP_RESPONSES {
            break;
        }
    }
    candidates.into_iter().collect()
}

fn routed_interfaces(
    scope: &ScanScope,
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Vec<Ipv4Addr> {
    let Ok(route_socket) = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)) else {
        return Vec::new();
    };
    routed_interfaces_with(scope, deadline, cancelled, |target| {
        crate::printer_network::route_interface(&route_socket, target)
    })
}

fn routed_interfaces_with(
    scope: &ScanScope,
    deadline: Instant,
    cancelled: &AtomicBool,
    mut resolve_route: impl FnMut(Ipv4Addr) -> Option<Ipv4Addr>,
) -> Vec<Ipv4Addr> {
    let mut interfaces = BTreeSet::new();
    for target in scope.addresses() {
        if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
            break;
        }
        if let Some(interface) = resolve_route(*target) {
            interfaces.insert(interface);
            if interfaces.len() > MAX_SCOPE_INTERFACES {
                return Vec::new();
            }
        }
    }
    interfaces.into_iter().collect()
}

fn search_interface(
    interface: Ipv4Addr,
    scope: &ScanScope,
    timeout: Duration,
    deadline: Instant,
    cancelled: &AtomicBool,
    ingress_budget: &mut SsdpIngressBudget,
    replies: &mut Vec<(Ipv4Addr, Vec<u8>)>,
) {
    let Ok(socket) = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) else {
        return;
    };
    let bind_address = SocketAddr::V4(SocketAddrV4::new(interface, 0));
    if socket.bind(&bind_address.into()).is_err()
        || socket.set_multicast_if_v4(&interface).is_err()
        || socket.set_multicast_ttl_v4(1).is_err()
        || socket.set_multicast_loop_v4(false).is_err()
    {
        return;
    }
    let socket: UdpSocket = socket.into();
    if socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .is_err()
        || cancelled.load(Ordering::Acquire)
        || Instant::now() >= deadline
        || socket
            .send_to(
                BAMBU_M_SEARCH,
                SocketAddr::V4(SocketAddrV4::new(BAMBU_SSDP_ADDRESS, BAMBU_SSDP_PORT)),
            )
            .is_err()
    {
        return;
    }

    let expires = Instant::now() + timeout;
    let mut buffer = [0u8; MAX_SSDP_PACKET_BYTES + 1];
    while !cancelled.load(Ordering::Acquire)
        && Instant::now() < deadline
        && Instant::now() < expires
        && replies.len() < MAX_SSDP_RESPONSES
        && ingress_budget.has_capacity()
    {
        match socket.recv_from(&mut buffer) {
            Ok((length, source)) => {
                let SocketAddr::V4(source) = source else {
                    continue;
                };
                if !ingress_budget.record(length) {
                    break;
                }
                if length > MAX_SSDP_PACKET_BYTES
                    || scope.addresses().binary_search(source.ip()).is_err()
                {
                    continue;
                }
                let packet = &buffer[..length];
                if parse_search_response(*source.ip(), packet).is_some() {
                    replies.push((*source.ip(), packet.to_vec()));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

fn parse_search_response(source: Ipv4Addr, packet: &[u8]) -> Option<BambuSsdpReply> {
    let header_end = packet.windows(4).position(|window| window == b"\r\n\r\n")?;
    if packet.len() > MAX_SSDP_PACKET_BYTES || header_end + 4 != packet.len() || !packet.is_ascii()
    {
        return None;
    }

    let response = std::str::from_utf8(packet).ok()?;
    let mut lines = response.split("\r\n");
    let status_line = lines.next()?.split_ascii_whitespace().collect::<Vec<_>>();
    if status_line.len() < 2
        || !matches!(status_line[0], "HTTP/1.0" | "HTTP/1.1")
        || status_line[1] != "200"
    {
        return None;
    }

    let mut search_target = None;
    let mut unique_service_name = None;
    let mut header_count = 0usize;
    for line in lines {
        if line.is_empty() {
            break;
        }
        header_count += 1;
        if header_count > 32 || line.len() > 512 {
            return None;
        }
        let (name, value) = line.split_once(':')?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return None;
        }
        let value = value.trim_matches([' ', '\t']);
        if value.is_empty() || value.chars().any(char::is_control) {
            return None;
        }
        match name.to_ascii_lowercase().as_str() {
            "st" if search_target.is_none() => search_target = Some(value),
            "st" => return None,
            "usn" if unique_service_name.is_none() => unique_service_name = Some(value),
            "usn" => return None,
            _ => {}
        }
    }

    if !search_target?.eq_ignore_ascii_case(BAMBU_SEARCH_TARGET) || unique_service_name.is_none() {
        return None;
    }

    Some(BambuSsdpReply { source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printer_network::parse_scan_scope;

    #[test]
    fn parses_a_bambu_search_response() {
        let packet = b"HTTP/1.1 200 OK\r\nST: urn:bambulab-com:device:3dprinter:1\r\nUSN: uuid:AB12CD34EF56::urn:bambulab-com:device:3dprinter:1\r\n\r\n";

        assert_eq!(
            parse_search_response(Ipv4Addr::new(192, 168, 1, 40), packet),
            Some(BambuSsdpReply {
                source: Ipv4Addr::new(192, 168, 1, 40),
            }),
        );
    }

    #[test]
    fn accepts_case_insensitive_headers_and_ignores_unneeded_location() {
        let packet = b"HTTP/1.0 200 OK\r\nlocation: http://192.168.1.40:80/description.xml\r\nuSn: uuid:AB12CD34EF56\r\nst: URN:BAMBULAB-COM:DEVICE:3DPRINTER:1\r\n\r\n";

        assert!(parse_search_response(Ipv4Addr::new(192, 168, 1, 40), packet).is_some());
    }

    #[test]
    fn rejects_non_success_wrong_service_and_untrusted_header_shapes() {
        let source = Ipv4Addr::new(192, 168, 1, 40);
        let wrong_status: &[u8] = b"HTTP/1.1 404 Not Found\r\nST: urn:bambulab-com:device:3dprinter:1\r\nUSN: uuid:1\r\n\r\n";
        let wrong_service: &[u8] = b"HTTP/1.1 200 OK\r\nST: upnp:rootdevice\r\nUSN: uuid:1\r\n\r\n";
        let missing_usn: &[u8] =
            b"HTTP/1.1 200 OK\r\nST: urn:bambulab-com:device:3dprinter:1\r\n\r\n";
        let duplicate_service: &[u8] = b"HTTP/1.1 200 OK\r\nST: urn:bambulab-com:device:3dprinter:1\r\nST: upnp:rootdevice\r\nUSN: uuid:1\r\n\r\n";
        let incomplete: &[u8] =
            b"HTTP/1.1 200 OK\nST: urn:bambulab-com:device:3dprinter:1\nUSN: uuid:1\n\n";
        let trailing_data: &[u8] = b"HTTP/1.1 200 OK\r\nST: urn:bambulab-com:device:3dprinter:1\r\nUSN: uuid:1\r\n\r\njunk\r\n\r\n";

        for packet in [
            wrong_status,
            wrong_service,
            missing_usn,
            duplicate_service,
            incomplete,
            trailing_data,
        ] {
            assert!(parse_search_response(source, packet).is_none());
        }
        assert!(parse_search_response(source, &vec![b'a'; MAX_SSDP_PACKET_BYTES + 1]).is_none());
    }

    #[test]
    fn candidates_are_deduplicated_and_filtered_to_the_selected_range() {
        let source = Ipv4Addr::new(192, 168, 1, 40);
        let outside = Ipv4Addr::new(192, 168, 1, 41);
        let scope = parse_scan_scope("192.168.1.40/32").unwrap();
        let valid =
            b"HTTP/1.1 200 OK\r\nST: urn:bambulab-com:device:3dprinter:1\r\nUSN: uuid:1\r\n\r\n"
                .to_vec();
        let invalid = b"HTTP/1.1 200 OK\r\nST: upnp:rootdevice\r\nUSN: uuid:1\r\n\r\n".to_vec();

        let candidates = validated_candidates(
            &scope,
            [
                (source, valid.clone()),
                (outside, valid.clone()),
                (source, invalid),
                (source, valid),
            ],
        );

        assert_eq!(candidates, vec![source]);
    }

    #[test]
    fn out_of_scope_responses_do_not_hide_later_in_scope_devices() {
        let source = Ipv4Addr::new(192, 168, 1, 40);
        let outside = Ipv4Addr::new(192, 168, 1, 41);
        let scope = parse_scan_scope("192.168.1.40/32").unwrap();
        let valid =
            b"HTTP/1.1 200 OK\r\nST: urn:bambulab-com:device:3dprinter:1\r\nUSN: uuid:1\r\n\r\n"
                .to_vec();
        let responses = std::iter::repeat_n((outside, valid.clone()), MAX_SSDP_RESPONSES + 1)
            .chain([(source, valid)]);

        assert_eq!(validated_candidates(&scope, responses), vec![source]);
    }

    #[test]
    fn caps_untrusted_datagram_count_before_selecting_candidates() {
        let source = Ipv4Addr::new(192, 168, 1, 40);
        let outside = Ipv4Addr::new(192, 168, 1, 41);
        let scope = parse_scan_scope("192.168.1.40/32").unwrap();
        let valid =
            b"HTTP/1.1 200 OK\r\nST: urn:bambulab-com:device:3dprinter:1\r\nUSN: uuid:1\r\n\r\n"
                .to_vec();
        let responses = std::iter::repeat_n((outside, vec![b'x']), MAX_SSDP_INGRESS_DATAGRAMS)
            .chain([(source, valid)]);

        assert!(validated_candidates(&scope, responses).is_empty());
    }

    #[test]
    fn resolves_every_selected_address_route_and_deduplicates_interfaces() {
        let scope = parse_scan_scope("192.168.1.10-192.168.1.12").unwrap();
        let first_interface = Ipv4Addr::new(192, 168, 1, 5);
        let second_interface = Ipv4Addr::new(10, 0, 0, 5);
        let mut visited = Vec::new();

        let interfaces = routed_interfaces_with(
            &scope,
            Instant::now() + Duration::from_secs(1),
            &AtomicBool::new(false),
            |target| {
                visited.push(target);
                Some(if target == Ipv4Addr::new(192, 168, 1, 10) {
                    first_interface
                } else {
                    second_interface
                })
            },
        );

        assert_eq!(
            visited,
            vec![
                Ipv4Addr::new(192, 168, 1, 10),
                Ipv4Addr::new(192, 168, 1, 11),
                Ipv4Addr::new(192, 168, 1, 12),
            ]
        );
        assert_eq!(interfaces, vec![second_interface, first_interface]);
    }

    #[test]
    fn cancellation_stops_route_resolution_before_multicast_setup() {
        let scope = parse_scan_scope("192.168.1.10/32").unwrap();
        let cancelled = AtomicBool::new(true);
        let interfaces = routed_interfaces_with(
            &scope,
            Instant::now() + Duration::from_secs(1),
            &cancelled,
            |_| panic!("cancelled route selection must not query another interface"),
        );

        assert!(interfaces.is_empty());
    }
}
