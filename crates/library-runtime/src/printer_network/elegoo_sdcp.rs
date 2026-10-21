use base64::Engine as _;
use library_core::models::PrinterJobStatus;
use ring::rand::SecureRandom as _;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SdcpRequestError {
    InvalidMainboardId,
    EncodingFailed,
    Timeout,
    Unreachable,
    InvalidHandshake,
    AcknowledgementMissing,
    Io,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SdcpStatusObservation {
    pub event_observed: bool,
    pub job_status: PrinterJobStatus,
}

impl SdcpStatusObservation {
    fn no_event() -> Self {
        Self {
            event_observed: false,
            job_status: PrinterJobStatus::Unknown,
        }
    }
}

const SDCP_PORT: u16 = 3030;
const MAX_HANDSHAKE_BYTES: usize = 16 * 1024;
const MAX_SERVER_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_SERVER_FRAMES: usize = 16;
const STATUS_EVENT_GRACE: Duration = Duration::from_millis(350);

pub(crate) fn build_status_refresh_request(
    mainboard_id: &str,
    message_id: uuid::Uuid,
    request_id: uuid::Uuid,
    timestamp: u64,
) -> Result<Vec<u8>, SdcpRequestError> {
    validate_mainboard_id(mainboard_id)?;

    let request = serde_json::json!({
        "Id": message_id.to_string(),
        "Data": {
            "Cmd": 0,
            "Data": {},
            "RequestID": request_id.to_string(),
            "MainboardID": mainboard_id,
            "TimeStamp": timestamp,
            "From": 0
        },
        "Topic": format!("sdcp/request/{mainboard_id}")
    });
    serde_json::to_vec(&request).map_err(|_| SdcpRequestError::EncodingFailed)
}

pub(crate) fn validate_mainboard_id(mainboard_id: &str) -> Result<(), SdcpRequestError> {
    if mainboard_id.is_empty()
        || mainboard_id.len() > 128
        || !mainboard_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        Err(SdcpRequestError::InvalidMainboardId)
    } else {
        Ok(())
    }
}

fn websocket_accept_value(client_key: &str) -> String {
    const WEBSOCKET_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut source = Vec::with_capacity(client_key.len() + WEBSOCKET_GUID.len());
    source.extend_from_slice(client_key.as_bytes());
    source.extend_from_slice(WEBSOCKET_GUID);
    let hash = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &source);
    base64::engine::general_purpose::STANDARD.encode(hash.as_ref())
}

fn build_websocket_upgrade_request(address: Ipv4Addr, client_key: &str) -> Vec<u8> {
    format!(
        "GET /websocket HTTP/1.1\r\nHost: {address}:{SDCP_PORT}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {client_key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
    .into_bytes()
}

fn validate_websocket_upgrade_response(
    response: &[u8],
    client_key: &str,
) -> Result<(), SdcpRequestError> {
    if response.len() > MAX_HANDSHAKE_BYTES || !response.ends_with(b"\r\n\r\n") {
        return Err(SdcpRequestError::InvalidHandshake);
    }
    let response = std::str::from_utf8(response).map_err(|_| SdcpRequestError::InvalidHandshake)?;
    let mut lines = response.split("\r\n");
    if lines.next() != Some("HTTP/1.1 101 Switching Protocols") {
        return Err(SdcpRequestError::InvalidHandshake);
    }
    let mut upgrade = false;
    let mut connection_upgrade = false;
    let mut accepted_key = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(SdcpRequestError::InvalidHandshake);
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "upgrade" => upgrade = value.trim().eq_ignore_ascii_case("websocket"),
            "connection" => {
                connection_upgrade = value
                    .split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("upgrade"));
            }
            "sec-websocket-accept" => accepted_key = Some(value.trim()),
            "sec-websocket-extensions" | "sec-websocket-protocol" => {
                return Err(SdcpRequestError::InvalidHandshake)
            }
            _ => {}
        }
    }
    if upgrade
        && connection_upgrade
        && accepted_key == Some(websocket_accept_value(client_key).as_str())
    {
        Ok(())
    } else {
        Err(SdcpRequestError::InvalidHandshake)
    }
}

/// Performs only the fixed, unauthenticated SDCP status-refresh exchange.
/// A successful return confirms a WebSocket upgrade and a correlated Cmd 0
/// acknowledgement. Any mapped job state comes from a matching status event
/// observed during this exchange; the event itself is not request-correlated.
pub(crate) fn send_status_refresh(
    address: Ipv4Addr,
    mainboard_id: &str,
    timeout: Duration,
    guard: &super::NetworkGuard,
) -> Result<SdcpStatusObservation, SdcpRequestError> {
    if !guard.permitted() {
        return Err(SdcpRequestError::Io);
    }
    if timeout.is_zero() {
        return Err(SdcpRequestError::Timeout);
    }
    validate_mainboard_id(mainboard_id)?;
    let deadline = Instant::now() + timeout;
    let socket = TcpStream::connect_timeout(
        &SocketAddrV4::new(address, SDCP_PORT).into(),
        remaining(deadline)?,
    )
    .map_err(map_io_error)?;
    perform_status_refresh(socket, address, mainboard_id, deadline)
}

fn perform_status_refresh(
    mut socket: TcpStream,
    address: Ipv4Addr,
    mainboard_id: &str,
    deadline: Instant,
) -> Result<SdcpStatusObservation, SdcpRequestError> {
    validate_mainboard_id(mainboard_id)?;
    let client_key =
        base64::engine::general_purpose::STANDARD.encode(uuid::Uuid::new_v4().as_bytes());
    socket.set_nodelay(true).map_err(map_io_error)?;
    socket
        .set_write_timeout(Some(remaining(deadline)?))
        .map_err(map_io_error)?;
    socket
        .write_all(&build_websocket_upgrade_request(address, &client_key))
        .map_err(map_io_error)?;

    let mut response = Vec::with_capacity(1024);
    let mut byte = [0; 1];
    while !response.windows(4).any(|window| window == b"\r\n\r\n") {
        if response.len() >= MAX_HANDSHAKE_BYTES {
            return Err(SdcpRequestError::InvalidHandshake);
        }
        socket
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(map_io_error)?;
        if socket.read(&mut byte).map_err(map_io_error)? == 0 {
            return Err(SdcpRequestError::InvalidHandshake);
        }
        response.push(byte[0]);
    }
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|offset| offset + 4)
        .ok_or(SdcpRequestError::InvalidHandshake)?;
    response.truncate(header_end);
    validate_websocket_upgrade_response(&response, &client_key)?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SdcpRequestError::Io)?
        .as_secs();
    let message_id = uuid::Uuid::new_v4();
    let request_id = uuid::Uuid::new_v4();
    let payload = build_status_refresh_request(mainboard_id, message_id, request_id, now)?;
    let mut mask = [0; 4];
    ring::rand::SystemRandom::new()
        .fill(&mut mask)
        .map_err(|_| SdcpRequestError::Io)?;
    let frame = encode_client_frame(0x1, &payload, mask)?;
    socket
        .set_write_timeout(Some(remaining(deadline)?))
        .map_err(map_io_error)?;
    socket.write_all(&frame).map_err(map_io_error)?;
    wait_for_status_ack(&mut socket, mainboard_id, request_id, deadline)
}

enum ServerFrame {
    Text(Vec<u8>),
    Ping(Vec<u8>),
    Pong,
    Close,
}

fn wait_for_status_ack(
    socket: &mut TcpStream,
    mainboard_id: &str,
    request_id: uuid::Uuid,
    deadline: Instant,
) -> Result<SdcpStatusObservation, SdcpRequestError> {
    let mut total_bytes = 0_usize;
    let mut acknowledgement_received = false;
    let mut observation = SdcpStatusObservation::no_event();
    let mut response_deadline = deadline;
    for _ in 0..MAX_SERVER_FRAMES {
        let frame = match read_server_frame(socket, response_deadline) {
            Err(SdcpRequestError::Timeout | SdcpRequestError::Unreachable) => {
                return if acknowledgement_received {
                    Ok(observation)
                } else {
                    Err(SdcpRequestError::AcknowledgementMissing)
                };
            }
            result => result?,
        };
        let frame_len = match &frame {
            ServerFrame::Text(payload) | ServerFrame::Ping(payload) => payload.len(),
            ServerFrame::Pong | ServerFrame::Close => 0,
        };
        total_bytes = total_bytes
            .checked_add(frame_len)
            .filter(|size| *size <= MAX_SERVER_RESPONSE_BYTES)
            .ok_or(SdcpRequestError::InvalidHandshake)?;

        match frame {
            ServerFrame::Text(payload) => {
                if is_correlated_status_ack(&payload, mainboard_id, request_id) {
                    if !acknowledgement_received {
                        acknowledgement_received = true;
                        response_deadline = deadline.min(Instant::now() + STATUS_EVENT_GRACE);
                    }
                    if observation.event_observed {
                        return Ok(observation);
                    }
                } else if let Some(job_status) = parse_status_event(&payload, mainboard_id) {
                    observation = SdcpStatusObservation {
                        event_observed: true,
                        job_status,
                    };
                    if acknowledgement_received {
                        return Ok(observation);
                    }
                }
            }
            ServerFrame::Ping(payload) => {
                let mut mask = [0; 4];
                ring::rand::SystemRandom::new()
                    .fill(&mut mask)
                    .map_err(|_| SdcpRequestError::Io)?;
                let pong = encode_client_frame(0xA, &payload, mask)?;
                let pong_deadline = if acknowledgement_received {
                    response_deadline
                } else {
                    deadline
                };
                let write_timeout = match remaining(pong_deadline) {
                    Ok(timeout) => timeout,
                    Err(SdcpRequestError::Timeout) if acknowledgement_received => {
                        return Ok(observation);
                    }
                    Err(error) => return Err(error),
                };
                socket
                    .set_write_timeout(Some(write_timeout))
                    .map_err(map_io_error)?;
                if let Err(error) = socket.write_all(&pong).map_err(map_io_error) {
                    if acknowledgement_received
                        && matches!(
                            error,
                            SdcpRequestError::Timeout | SdcpRequestError::Unreachable
                        )
                    {
                        return Ok(observation);
                    }
                    return Err(error);
                }
            }
            ServerFrame::Pong => {}
            ServerFrame::Close => {
                return if acknowledgement_received {
                    Ok(observation)
                } else {
                    Err(SdcpRequestError::AcknowledgementMissing)
                };
            }
        }
    }
    if acknowledgement_received {
        Ok(observation)
    } else {
        Err(SdcpRequestError::AcknowledgementMissing)
    }
}

fn read_server_frame(
    socket: &mut TcpStream,
    deadline: Instant,
) -> Result<ServerFrame, SdcpRequestError> {
    let mut header = [0; 2];
    read_exact_until(socket, &mut header, deadline)?;
    let fin = header[0] & 0x80 != 0;
    let reserved = header[0] & 0x70;
    let opcode = header[0] & 0x0f;
    let masked = header[1] & 0x80 != 0;
    let is_control = opcode & 0x08 != 0;
    if !fin || reserved != 0 || masked || !matches!(opcode, 0x1 | 0x8..=0xA) {
        return Err(SdcpRequestError::InvalidHandshake);
    }

    let length_marker = header[1] & 0x7f;
    let payload_len = match length_marker {
        length @ 0..=125 => usize::from(length),
        126 => {
            let mut extended = [0; 2];
            read_exact_until(socket, &mut extended, deadline)?;
            let length = usize::from(u16::from_be_bytes(extended));
            if length < 126 {
                return Err(SdcpRequestError::InvalidHandshake);
            }
            length
        }
        127 => {
            let mut extended = [0; 8];
            read_exact_until(socket, &mut extended, deadline)?;
            let length = u64::from_be_bytes(extended);
            if length < 65_536 || length & (1 << 63) != 0 {
                return Err(SdcpRequestError::InvalidHandshake);
            }
            usize::try_from(length).map_err(|_| SdcpRequestError::InvalidHandshake)?
        }
        _ => return Err(SdcpRequestError::InvalidHandshake),
    };
    if payload_len > MAX_SERVER_RESPONSE_BYTES || (is_control && payload_len > 125) {
        return Err(SdcpRequestError::InvalidHandshake);
    }
    let mut payload = vec![0; payload_len];
    read_exact_until(socket, &mut payload, deadline)?;
    match opcode {
        0x1 => Ok(ServerFrame::Text(payload)),
        0x8 => Ok(ServerFrame::Close),
        0x9 => Ok(ServerFrame::Ping(payload)),
        0xA => Ok(ServerFrame::Pong),
        _ => Err(SdcpRequestError::InvalidHandshake),
    }
}

fn read_exact_until(
    socket: &mut TcpStream,
    buffer: &mut [u8],
    deadline: Instant,
) -> Result<(), SdcpRequestError> {
    let mut offset = 0;
    while offset < buffer.len() {
        socket
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(map_io_error)?;
        let count = socket.read(&mut buffer[offset..]).map_err(map_io_error)?;
        if count == 0 {
            return Err(SdcpRequestError::Io);
        }
        offset += count;
    }
    remaining(deadline)?;
    Ok(())
}

fn is_correlated_status_ack(payload: &[u8], mainboard_id: &str, request_id: uuid::Uuid) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) else {
        return false;
    };
    value.get("Topic").and_then(serde_json::Value::as_str)
        == Some(format!("sdcp/response/{mainboard_id}").as_str())
        && value
            .pointer("/Data/Cmd")
            .and_then(serde_json::Value::as_u64)
            == Some(0)
        && value
            .pointer("/Data/MainboardID")
            .and_then(serde_json::Value::as_str)
            == Some(mainboard_id)
        && value
            .pointer("/Data/RequestID")
            .and_then(serde_json::Value::as_str)
            == Some(request_id.to_string().as_str())
        && value
            .pointer("/Data/Data/Ack")
            .and_then(serde_json::Value::as_i64)
            == Some(0)
}

// Original CC1 status envelope and print states are defined by Elegoo's FDM CC adapter:
// https://github.com/elegooofficial/elegoo-link/blob/46c7b814e055cf9675d58482d79f43d0bd2280da/src/lan/adapters/elegoo_fdm_cc/elegoo_fdm_cc_message_adapter.cpp#L33-L62
// The same adapter parses the top-level Status object (handlePrinterStatus).
// Only directly represented job states are mapped; transitional and unknown values stay Unknown.
fn parse_status_event(payload: &[u8], mainboard_id: &str) -> Option<PrinterJobStatus> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    if value.get("Topic").and_then(serde_json::Value::as_str)
        != Some(format!("sdcp/status/{mainboard_id}").as_str())
        || value.get("MainboardID").and_then(serde_json::Value::as_str) != Some(mainboard_id)
        || !value
            .get("Status")
            .is_some_and(serde_json::Value::is_object)
    {
        return None;
    }
    Some(
        match value
            .pointer("/Status/PrintInfo/Status")
            .and_then(serde_json::Value::as_u64)
        {
            Some(0) => PrinterJobStatus::Idle,
            Some(6) => PrinterJobStatus::Paused,
            Some(9) => PrinterJobStatus::Complete,
            Some(13) => PrinterJobStatus::Printing,
            _ => PrinterJobStatus::Unknown,
        },
    )
}

fn remaining(deadline: Instant) -> Result<Duration, SdcpRequestError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(SdcpRequestError::Timeout)
    } else {
        Ok(remaining)
    }
}

fn map_io_error(error: std::io::Error) -> SdcpRequestError {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => SdcpRequestError::Timeout,
        std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::NotConnected
        | std::io::ErrorKind::AddrNotAvailable => SdcpRequestError::Unreachable,
        _ => SdcpRequestError::Io,
    }
}

fn encode_client_frame(
    opcode: u8,
    payload: &[u8],
    mask_key: [u8; 4],
) -> Result<Vec<u8>, SdcpRequestError> {
    const MAX_CLIENT_PAYLOAD: usize = 64 * 1024;
    let is_control = matches!(opcode, 0x8..=0xA);
    if !matches!(opcode, 0x1 | 0x8..=0xA)
        || payload.len() > MAX_CLIENT_PAYLOAD
        || (is_control && payload.len() > 125)
    {
        return Err(SdcpRequestError::EncodingFailed);
    }

    let mut frame = Vec::with_capacity(2 + 8 + mask_key.len() + payload.len());
    frame.push(0x80 | opcode);
    match payload.len() {
        length @ 0..=125 => frame.push(0x80 | length as u8),
        length @ 126..=65_535 => {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        }
        _ => return Err(SdcpRequestError::EncodingFailed),
    }
    frame.extend_from_slice(&mask_key);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask_key[index % mask_key.len()]),
    );
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::io::Read;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn status_refresh_request_uses_only_the_read_only_status_command() {
        let bytes = build_status_refresh_request(
            "AB12CD34EF56",
            uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap(),
            uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap(),
            1_792_101_000,
        )
        .expect("valid SDCP identifiers should produce a request");
        let request: Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(
            request,
            json!({
                "Id": "00000000-0000-4000-8000-000000000001",
                "Data": {
                    "Cmd": 0,
                    "Data": {},
                    "RequestID": "00000000-0000-4000-8000-000000000002",
                    "MainboardID": "AB12CD34EF56",
                    "TimeStamp": 1_792_101_000_u64,
                    "From": 0
                },
                "Topic": "sdcp/request/AB12CD34EF56"
            })
        );
    }

    #[test]
    fn status_refresh_request_rejects_topic_injection() {
        assert_eq!(
            build_status_refresh_request(
                "AB12CD34EF56/other",
                uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap(),
                uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap(),
                1_792_101_000,
            ),
            Err(SdcpRequestError::InvalidMainboardId)
        );
    }

    #[test]
    fn websocket_accept_value_matches_the_rfc6455_example() {
        assert_eq!(
            websocket_accept_value("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn websocket_upgrade_request_uses_only_the_fixed_sdcp_path() {
        let request = String::from_utf8(build_websocket_upgrade_request(
            Ipv4Addr::new(192, 168, 0, 108),
            "dGhlIHNhbXBsZSBub25jZQ==",
        ))
        .unwrap();

        assert!(request.starts_with("GET /websocket HTTP/1.1\r\n"));
        assert!(request.contains("Host: 192.168.0.108:3030\r\n"));
        assert!(request.contains("Sec-WebSocket-Version: 13\r\n"));
        assert!(!request.contains("Authorization:"));
        assert!(!request.contains("Origin:"));
    }

    #[test]
    fn websocket_upgrade_rejects_wrong_accept_and_unsolicited_extensions() {
        let wrong_accept = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: wrong\r\n\r\n";
        assert_eq!(
            validate_websocket_upgrade_response(wrong_accept, "key"),
            Err(SdcpRequestError::InvalidHandshake)
        );

        let mut with_extension = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\nSec-WebSocket-Extensions: permessage-deflate\r\n\r\n",
            websocket_accept_value("key")
        )
        .into_bytes();
        assert_eq!(
            validate_websocket_upgrade_response(&with_extension, "key"),
            Err(SdcpRequestError::InvalidHandshake)
        );
        with_extension.clear();
    }

    #[test]
    fn sdcp_ack_must_match_topic_command_mainboard_and_request_id() {
        let request_id = uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap();
        let ack = json!({
            "Topic": "sdcp/response/AB12CD34EF56",
            "Data": {
                "Cmd": 0,
                "Data": {"Ack": 0},
                "RequestID": request_id.to_string(),
                "MainboardID": "AB12CD34EF56"
            }
        });
        assert!(is_correlated_status_ack(
            &serde_json::to_vec(&ack).unwrap(),
            "AB12CD34EF56",
            request_id
        ));

        let mut wrong_id = ack.clone();
        wrong_id["Data"]["RequestID"] = json!(uuid::Uuid::new_v4().to_string());
        assert!(!is_correlated_status_ack(
            &serde_json::to_vec(&wrong_id).unwrap(),
            "AB12CD34EF56",
            request_id
        ));
        assert!(!is_correlated_status_ack(
            &serde_json::to_vec(&ack).unwrap(),
            "OTHERBOARD",
            request_id
        ));
    }

    #[test]
    fn status_event_requires_matching_topic_and_top_level_mainboard_and_status_object() {
        let valid = json!({
            "Topic": "sdcp/status/AB12CD34EF56",
            "MainboardID": "AB12CD34EF56",
            "Status": {"PrintInfo": {"Status": 13}}
        });
        assert_eq!(
            parse_status_event(&serde_json::to_vec(&valid).unwrap(), "AB12CD34EF56"),
            Some(PrinterJobStatus::Printing)
        );

        let mut wrong_topic = valid.clone();
        wrong_topic["Topic"] = json!("sdcp/status/OTHER");
        assert!(
            parse_status_event(&serde_json::to_vec(&wrong_topic).unwrap(), "AB12CD34EF56")
                .is_none()
        );
        let mut wrong_command = valid.clone();
        wrong_command["Status"] = json!(null);
        assert!(
            parse_status_event(&serde_json::to_vec(&wrong_command).unwrap(), "AB12CD34EF56")
                .is_none()
        );
        let mut wrong_board = valid.clone();
        wrong_board["MainboardID"] = json!("OTHER");
        assert!(
            parse_status_event(&serde_json::to_vec(&wrong_board).unwrap(), "AB12CD34EF56")
                .is_none()
        );
    }

    #[test]
    fn original_cc1_job_states_map_only_directly_represented_states() {
        for (code, expected) in [
            (0, PrinterJobStatus::Idle),
            (6, PrinterJobStatus::Paused),
            (9, PrinterJobStatus::Complete),
            (13, PrinterJobStatus::Printing),
        ] {
            let event = status_fixture(json!(code));
            assert_eq!(
                parse_status_event(&serde_json::to_vec(&event).unwrap(), "AB12CD34EF56"),
                Some(expected)
            );
        }
        for code in [
            1, 5, 7, 8, 10, 11, 12, 14, 15, 16, 17, 18, 19, 20, 21, 22, 255,
        ] {
            let event = status_fixture(json!(code));
            assert_eq!(
                parse_status_event(&serde_json::to_vec(&event).unwrap(), "AB12CD34EF56"),
                Some(PrinterJobStatus::Unknown)
            );
        }
    }

    #[test]
    fn unsupported_or_missing_print_info_is_an_observed_event_with_unknown_job() {
        for status in [
            json!("13"),
            json!(13.0),
            json!(-1),
            json!(null),
            json!(true),
            json!({}),
            json!([]),
        ] {
            let event = status_fixture(status);
            assert_eq!(
                parse_status_event(&serde_json::to_vec(&event).unwrap(), "AB12CD34EF56"),
                Some(PrinterJobStatus::Unknown)
            );
        }
        for status in [
            json!({}),
            json!({"PrintInfo": {}}),
            json!({"PrintInfo": null}),
        ] {
            let mut event = status_fixture(json!(13));
            event["Status"] = status;
            assert_eq!(
                parse_status_event(&serde_json::to_vec(&event).unwrap(), "AB12CD34EF56"),
                Some(PrinterJobStatus::Unknown)
            );
        }
    }

    #[test]
    fn nested_command_envelope_does_not_count_as_a_status_event() {
        let event = json!({
            "Topic": "sdcp/status/AB12CD34EF56",
            "Data": {"Cmd": 0, "MainboardID": "AB12CD34EF56", "Data": {"PrintInfo": {"Status": 13}}}
        });
        assert_eq!(
            parse_status_event(&serde_json::to_vec(&event).unwrap(), "AB12CD34EF56"),
            None
        );
        assert_eq!(parse_status_event(b"not json", "AB12CD34EF56"), None);
    }

    fn status_fixture(status: Value) -> Value {
        json!({
            "Topic": "sdcp/status/AB12CD34EF56",
            "MainboardID": "AB12CD34EF56",
            "Status": {"PrintInfo": {"Status": status}}
        })
    }

    fn ack_fixture(request_id: uuid::Uuid) -> Value {
        json!({
            "Topic": "sdcp/response/AB12CD34EF56",
            "Data": {
                "Cmd": 0,
                "Data": {"Ack": 0},
                "RequestID": request_id.to_string(),
                "MainboardID": "AB12CD34EF56"
            }
        })
    }

    fn receive_fixture_frames(
        frames: Vec<Value>,
    ) -> Result<SdcpStatusObservation, SdcpRequestError> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let socket_address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            for frame in frames {
                socket
                    .write_all(&encode_server_text_frame(
                        &serde_json::to_vec(&frame).unwrap(),
                    ))
                    .unwrap();
            }
            // Explicit WebSocket close avoids depending on TCP EOF classification.
            socket.write_all(&[0x88, 0]).unwrap();
        });
        let mut socket = TcpStream::connect(socket_address).unwrap();
        let result = wait_for_status_ack(
            &mut socket,
            "AB12CD34EF56",
            uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap(),
            Instant::now() + Duration::from_secs(1),
        );
        server.join().unwrap();
        result
    }

    #[test]
    fn mapped_status_requires_ack_and_supports_both_frame_orders() {
        let request_id = uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap();
        for frames in [
            vec![status_fixture(json!(6)), ack_fixture(request_id)],
            vec![ack_fixture(request_id), status_fixture(json!(6))],
        ] {
            assert_eq!(
                receive_fixture_frames(frames),
                Ok(SdcpStatusObservation {
                    event_observed: true,
                    job_status: PrinterJobStatus::Paused,
                })
            );
        }
        assert_eq!(
            receive_fixture_frames(vec![status_fixture(json!(13))]),
            Err(SdcpRequestError::AcknowledgementMissing)
        );
        let mut wrong_ack = ack_fixture(request_id);
        wrong_ack["Data"]["Data"]["Ack"] = json!(1);
        assert_eq!(
            receive_fixture_frames(vec![status_fixture(json!(13)), wrong_ack]),
            Err(SdcpRequestError::AcknowledgementMissing)
        );
    }

    #[test]
    fn wrong_identity_events_never_supply_a_job_state_even_with_a_valid_ack() {
        let request_id = uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap();
        let mut wrong_board = status_fixture(json!(13));
        wrong_board["MainboardID"] = json!("OTHER");
        let mut wrong_topic = status_fixture(json!(13));
        wrong_topic["Topic"] = json!("sdcp/status/OTHER");
        assert_eq!(
            receive_fixture_frames(vec![wrong_board, wrong_topic, ack_fixture(request_id)]),
            Ok(SdcpStatusObservation::no_event())
        );
    }

    #[test]
    fn uncorrelated_ack_does_not_complete_the_status_refresh() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let socket_address = listener.local_addr().unwrap();
        let expected_request_id =
            uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap();
        let wrong_request_id =
            uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000003").unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let ack = json!({
                "Topic": "sdcp/response/AB12CD34EF56",
                "Data": {
                    "Cmd": 0,
                    "Data": {"Ack": 0},
                    "RequestID": wrong_request_id.to_string(),
                    "MainboardID": "AB12CD34EF56"
                }
            });
            socket
                .write_all(&encode_server_text_frame(
                    &serde_json::to_vec(&ack).unwrap(),
                ))
                .unwrap();
            thread::sleep(Duration::from_millis(125));
        });

        let mut socket = TcpStream::connect(socket_address).unwrap();
        assert_eq!(
            wait_for_status_ack(
                &mut socket,
                "AB12CD34EF56",
                expected_request_id,
                Instant::now() + Duration::from_millis(50),
            ),
            Err(SdcpRequestError::AcknowledgementMissing)
        );
        server.join().unwrap();
    }

    #[test]
    fn server_frame_read_obeys_the_absolute_deadline_for_trickled_bytes() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let socket_address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.write_all(&[0x81, 3]).unwrap();
            socket.write_all(b"a").unwrap();
            thread::sleep(Duration::from_millis(60));
            socket.write_all(b"b").unwrap();
            thread::sleep(Duration::from_millis(60));
            socket.write_all(b"c").unwrap();
        });

        let mut socket = TcpStream::connect(socket_address).unwrap();
        assert!(matches!(
            read_server_frame(&mut socket, Instant::now() + Duration::from_millis(100)),
            Err(SdcpRequestError::Timeout)
        ));
        server.join().unwrap();
    }

    #[test]
    fn exact_reader_checks_deadline_after_consuming_the_requested_bytes() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let socket_address = listener.local_addr().unwrap();
        let server = thread::spawn(move || listener.accept().unwrap());
        let mut socket = TcpStream::connect(socket_address).unwrap();
        let (_peer, _) = server.join().unwrap();

        assert_eq!(
            read_exact_until(
                &mut socket,
                &mut [],
                Instant::now() - Duration::from_millis(1)
            ),
            Err(SdcpRequestError::Timeout)
        );
    }

    #[test]
    fn loopback_websocket_exchange_sends_only_the_sdcp_status_refresh() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let socket_address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0; 1];
            while !request.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() <= MAX_HANDSHAKE_BYTES);
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("GET /websocket HTTP/1.1\r\n"));
            assert!(request.contains("Host: 192.168.0.108:3030\r\n"));
            assert!(!request.contains("Authorization:"));
            let key = request
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("Sec-WebSocket-Key"))
                        .map(|(_, value)| value.trim())
                })
                .unwrap();
            let accept = websocket_accept_value(key);
            write!(
                socket,
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
            )
            .unwrap();

            let mut frame_header = [0; 2];
            socket.read_exact(&mut frame_header).unwrap();
            assert_eq!(frame_header[0], 0x81);
            assert_ne!(frame_header[1] & 0x80, 0);
            let payload_len = match frame_header[1] & 0x7f {
                length @ 0..=125 => usize::from(length),
                126 => {
                    let mut extended = [0; 2];
                    socket.read_exact(&mut extended).unwrap();
                    usize::from(u16::from_be_bytes(extended))
                }
                _ => panic!("unexpectedly large SDCP request frame"),
            };
            assert!(payload_len <= MAX_CLIENT_PAYLOAD_FOR_TEST);
            let mut mask = [0; 4];
            socket.read_exact(&mut mask).unwrap();
            let mut payload = vec![0; payload_len];
            socket.read_exact(&mut payload).unwrap();
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % mask.len()];
            }
            let request: Value = serde_json::from_slice(&payload).unwrap();
            assert_eq!(request["Topic"], "sdcp/request/AB12CD34EF56");
            assert_eq!(request["Data"]["Cmd"], 0);
            assert_eq!(request["Data"]["Data"], json!({}));
            assert_eq!(request["Data"]["MainboardID"], "AB12CD34EF56");
            assert_eq!(request["Data"]["From"], 0);
            let request_id = request["Data"]["RequestID"].as_str().unwrap();
            let status = json!({
                "Id": request["Id"],
                "Status": {"PrintInfo": {"Status": 13}},
                "MainboardID": "AB12CD34EF56",
                "TimeStamp": 1_792_101_000_u64,
                "Topic": "sdcp/status/AB12CD34EF56"
            });
            socket
                .write_all(&encode_server_text_frame(
                    &serde_json::to_vec(&status).unwrap(),
                ))
                .unwrap();
            let ack = json!({
                "Id": request["Id"],
                "Data": {
                    "Cmd": 0,
                    "Data": {"Ack": 0},
                    "RequestID": request_id,
                    "MainboardID": "AB12CD34EF56",
                    "TimeStamp": 1_792_101_000_u64
                },
                "Topic": "sdcp/response/AB12CD34EF56"
            });
            socket
                .write_all(&encode_server_text_frame(
                    &serde_json::to_vec(&ack).unwrap(),
                ))
                .unwrap();
        });

        let stream = TcpStream::connect(socket_address).unwrap();
        let status_received = perform_status_refresh(
            stream,
            Ipv4Addr::new(192, 168, 0, 108),
            "AB12CD34EF56",
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
        assert!(status_received.event_observed);
        assert_eq!(status_received.job_status, PrinterJobStatus::Printing);
        server.join().unwrap();
    }

    #[test]
    fn acknowledgement_without_a_status_event_succeeds_with_unknown_status() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let socket_address = listener.local_addr().unwrap();
        let request_id = uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let ack = json!({
                "Topic": "sdcp/response/AB12CD34EF56",
                "Data": {
                    "Cmd": 0,
                    "Data": {"Ack": 0},
                    "RequestID": request_id.to_string(),
                    "MainboardID": "AB12CD34EF56"
                }
            });
            socket
                .write_all(&encode_server_text_frame(
                    &serde_json::to_vec(&ack).unwrap(),
                ))
                .unwrap();
            thread::sleep(STATUS_EVENT_GRACE + Duration::from_millis(50));
        });

        let mut socket = TcpStream::connect(socket_address).unwrap();
        assert_eq!(
            wait_for_status_ack(
                &mut socket,
                "AB12CD34EF56",
                request_id,
                Instant::now() + Duration::from_secs(1),
            ),
            Ok(SdcpStatusObservation::no_event())
        );
        server.join().unwrap();
    }

    const MAX_CLIENT_PAYLOAD_FOR_TEST: usize = 64 * 1024;

    fn encode_server_text_frame(payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![0x81];
        match payload.len() {
            length @ 0..=125 => frame.push(length as u8),
            length @ 126..=65_535 => {
                frame.push(126);
                frame.extend_from_slice(&(length as u16).to_be_bytes());
            }
            length => {
                frame.push(127);
                frame.extend_from_slice(&(length as u64).to_be_bytes());
            }
        }
        frame.extend_from_slice(payload);
        frame
    }

    #[test]
    fn client_text_frame_masks_each_payload_byte() {
        assert_eq!(
            encode_client_frame(0x1, b"test", [1, 2, 3, 4]).unwrap(),
            vec![
                0x81,
                0x84,
                1,
                2,
                3,
                4,
                b't' ^ 1,
                b'e' ^ 2,
                b's' ^ 3,
                b't' ^ 4
            ]
        );
    }

    #[test]
    fn client_text_frame_encodes_extended_length_and_masks_payload() {
        let payload = vec![b'x'; 126];
        let frame = encode_client_frame(0x1, &payload, [0x11, 0x22, 0x33, 0x44]).unwrap();

        assert_eq!(&frame[..4], &[0x81, 0xFE, 0x00, 0x7E]);
        assert_eq!(&frame[4..8], &[0x11, 0x22, 0x33, 0x44]);
        assert!(frame[8..]
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte == (b'x' ^ [0x11, 0x22, 0x33, 0x44][index % 4])));
    }

    #[test]
    fn client_frame_rejects_unsupported_opcodes_and_oversized_control_payloads() {
        assert_eq!(
            encode_client_frame(0x2, b"binary", [1, 2, 3, 4]),
            Err(SdcpRequestError::EncodingFailed)
        );
        assert_eq!(
            encode_client_frame(0x9, &[0; 126], [1, 2, 3, 4]),
            Err(SdcpRequestError::EncodingFailed)
        );
        assert_eq!(
            encode_client_frame(0x1, &[0; 65_537], [1, 2, 3, 4]),
            Err(SdcpRequestError::EncodingFailed)
        );
    }
}
