use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

const SDCP_DISCOVERY_REQUEST: &[u8] = b"M99999";
const SDCP_DISCOVERY_PORT: u16 = 3000;
const MAX_DISCOVERY_PACKET_BYTES: usize = 4 * 1024;
const MAX_DISCOVERY_DATAGRAMS: usize = 16;
const MAX_DISCOVERY_TOTAL_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiscoveryError {
    Timeout,
    Unreachable,
    InvalidResponse,
    Failed,
}

fn discovery_request() -> &'static [u8] {
    SDCP_DISCOVERY_REQUEST
}

fn parse_discovery_response(source: Ipv4Addr, target: Ipv4Addr, packet: &[u8]) -> Option<String> {
    if source != target || packet.is_empty() || packet.len() > MAX_DISCOVERY_PACKET_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(packet).ok()?;
    let data = value.get("Data")?.as_object()?;
    let response_ip = data
        .get("MainboardIP")?
        .as_str()?
        .parse::<Ipv4Addr>()
        .ok()?;
    if response_ip != target {
        return None;
    }
    let mainboard_id = data.get("MainboardID")?.as_str()?;
    super::elegoo_sdcp::validate_mainboard_id(mainboard_id).ok()?;
    Some(mainboard_id.to_owned())
}

pub(crate) fn discover_mainboard_id(
    target: Ipv4Addr,
    timeout: Duration,
    guard: &super::NetworkGuard,
) -> Result<String, DiscoveryError> {
    if !guard.permitted() {
        return Err(DiscoveryError::Failed);
    }
    if timeout.is_zero() {
        return Err(DiscoveryError::Timeout);
    }
    let deadline = Instant::now() + timeout;
    let route_socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|_| DiscoveryError::Failed)?;
    let interface =
        super::scope::route_interface(&route_socket, target).ok_or(DiscoveryError::Unreachable)?;
    if !super::scope::is_private_unicast(interface) {
        return Err(DiscoveryError::Unreachable);
    }
    let socket =
        UdpSocket::bind(SocketAddrV4::new(interface, 0)).map_err(|error| map_io_error(&error))?;
    socket
        .set_broadcast(true)
        .map_err(|error| map_io_error(&error))?;
    if deadline.saturating_duration_since(Instant::now()).is_zero() {
        return Err(DiscoveryError::Timeout);
    }
    socket
        .send_to(
            discovery_request(),
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::BROADCAST, SDCP_DISCOVERY_PORT)),
        )
        .map_err(|error| map_io_error(&error))?;

    let mut datagrams = 0_usize;
    let mut total_bytes = 0_usize;
    let mut buffer = [0_u8; MAX_DISCOVERY_PACKET_BYTES + 1];
    while datagrams < MAX_DISCOVERY_DATAGRAMS && total_bytes < MAX_DISCOVERY_TOTAL_BYTES {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(DiscoveryError::Timeout);
        }
        socket
            .set_read_timeout(Some(remaining.min(Duration::from_millis(100))))
            .map_err(|error| map_io_error(&error))?;
        match socket.recv_from(&mut buffer) {
            Ok((length, source)) => {
                datagrams = datagrams.saturating_add(1);
                total_bytes = total_bytes.saturating_add(length);
                if length > MAX_DISCOVERY_PACKET_BYTES || total_bytes > MAX_DISCOVERY_TOTAL_BYTES {
                    return Err(DiscoveryError::InvalidResponse);
                }
                if let SocketAddr::V4(source) = source {
                    if let Some(mainboard_id) =
                        parse_discovery_response(*source.ip(), target, &buffer[..length])
                    {
                        return Ok(mainboard_id);
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(map_io_error(&error)),
        }
    }
    Err(DiscoveryError::Timeout)
}

fn map_io_error(error: &io::Error) -> DiscoveryError {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => DiscoveryError::Timeout,
        io::ErrorKind::ConnectionRefused
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::NotConnected
        | io::ErrorKind::AddrNotAvailable
        | io::ErrorKind::NetworkUnreachable => DiscoveryError::Unreachable,
        _ => DiscoveryError::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TARGET: Ipv4Addr = Ipv4Addr::new(192, 168, 0, 108);

    fn response(mainboard_ip: &str, mainboard_id: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "Data": {
                "MainboardIP": mainboard_ip,
                "MainboardID": mainboard_id
            }
        }))
        .unwrap()
    }

    #[test]
    fn discovery_uses_only_the_documented_sdcp_request() {
        assert_eq!(discovery_request(), b"M99999");
    }

    #[test]
    fn accepts_only_a_valid_mainboard_id_from_the_saved_target() {
        assert_eq!(
            parse_discovery_response(TARGET, TARGET, &response("192.168.0.108", "AB12CD34EF56")),
            Some("AB12CD34EF56".into())
        );
        assert!(parse_discovery_response(
            Ipv4Addr::new(192, 168, 0, 109),
            TARGET,
            &response("192.168.0.108", "AB12CD34EF56")
        )
        .is_none());
        assert!(parse_discovery_response(
            TARGET,
            TARGET,
            &response("192.168.0.109", "AB12CD34EF56")
        )
        .is_none());
        assert!(parse_discovery_response(
            TARGET,
            TARGET,
            &response("192.168.0.108", "AB12CD34EF56/other")
        )
        .is_none());
    }

    #[test]
    fn discovery_response_is_bounded_and_must_be_json_with_required_fields() {
        assert!(parse_discovery_response(TARGET, TARGET, b"not json").is_none());
        assert!(parse_discovery_response(TARGET, TARGET, br#"{"Data":{}}"#).is_none());
        assert!(parse_discovery_response(
            TARGET,
            TARGET,
            &vec![b'x'; MAX_DISCOVERY_PACKET_BYTES + 1]
        )
        .is_none());
    }
}
