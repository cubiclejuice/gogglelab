//! Shared, bounded read-only printer transports. Hosts must supply a live permission guard.
mod bambu_discovery;
mod elegoo_discovery;
mod elegoo_sdcp;
pub mod scope;

/// Checked at construction and immediately before every network entry point.
/// No permissive default is provided; the host owns the policy and may revoke it.
#[derive(Clone)]
pub struct NetworkGuard(Arc<dyn Fn() -> bool + Send + Sync>);
impl NetworkGuard {
    pub fn new(check: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(check))
    }
    fn permitted(&self) -> bool {
        (self.0)()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use library_core::models::PrinterSourceKind;
    use std::sync::atomic::AtomicUsize;

    #[derive(Clone)]
    enum FakeMode {
        Bambu,
        BambuOutOfScope,
        Elegoo,
        ElegooUnknown,
        ElegooNoEvent,
        Moonraker,
        MoonrakerAuth(&'static str),
        PrusaAuth,
        PrusaLink,
        PrusaVersionAuth,
        LargeResponse,
        CancelAfterFirst(Arc<AtomicBool>),
    }

    #[derive(Default)]
    struct FakeTransport {
        mode: Option<FakeMode>,
        requests: Mutex<Vec<ProbeRequest>>,
        bambu_hosts: Mutex<Vec<Ipv4Addr>>,
        ssdp_requests: AtomicUsize,
        elegoo_discoveries: Mutex<Vec<(Ipv4Addr, Duration)>>,
        elegoo_requests: Mutex<Vec<(Ipv4Addr, String, Duration)>>,
        active_requests: AtomicUsize,
        maximum_timeout: Mutex<Duration>,
    }

    impl FakeTransport {
        fn new(mode: FakeMode) -> Self {
            Self {
                mode: Some(mode),
                ..Self::default()
            }
        }
    }

    impl ProbeTransport for FakeTransport {
        fn get(
            &self,
            request: ProbeRequest,
            timeout: Duration,
        ) -> Result<ProbeResponse, ProbeError> {
            let active = self.active_requests.fetch_add(1, Ordering::SeqCst) + 1;
            assert!(active <= MAX_SCAN_WORKERS);
            self.active_requests.fetch_sub(1, Ordering::SeqCst);
            let mut maximum = self.maximum_timeout.lock().unwrap();
            *maximum = (*maximum).max(timeout);
            drop(maximum);

            let mut requests = self.requests.lock().unwrap();
            requests.push(request.clone());
            let first = requests.len() == 1;
            drop(requests);

            match self.mode.as_ref().unwrap() {
                FakeMode::Moonraker => match request.path {
                    "/server/info" => json_response(
                        200,
                        br#"{"result":{"klippy_state":"ready","moonraker_version":"v1"}}"#,
                    ),
                    "/printer/info" => {
                        json_response(200, br#"{"result":{"state":"ready"}}"#)
                    }
                    "/printer/objects/query?print_stats&webhooks" => json_response(
                        200,
                        br#"{"result":{"status":{"print_stats":{"state":"printing"},"webhooks":{"state":"ready"}}}}"#,
                    ),
                    _ => Ok(ProbeResponse::new(404, Vec::new())),
                },
                FakeMode::MoonrakerAuth(auth_path) if request.path == *auth_path => {
                    Ok(ProbeResponse::new(401, Vec::new()))
                }
                FakeMode::MoonrakerAuth(_) => match request.path {
                    "/server/info" => json_response(
                        200,
                        br#"{"result":{"klippy_state":"ready","moonraker_version":"v1"}}"#,
                    ),
                    "/printer/info" => json_response(200, br#"{"result":{"state":"ready"}}"#),
                    _ => Ok(ProbeResponse::new(404, Vec::new())),
                },
                FakeMode::PrusaAuth => match request.path {
                    "/api/version" => json_response(
                        200,
                        br#"{"api":"2.0","text":"PrusaLink","version":"0.7.2"}"#,
                    ),
                    "/api/v1/status" => Ok(ProbeResponse::new(401, Vec::new())),
                    _ => Ok(ProbeResponse::new(404, Vec::new())),
                },
                FakeMode::PrusaLink => match request.path {
                    "/api/version" => json_response(
                        200,
                        br#"{"api":"2.0","text":"PrusaLink","version":"0.7.2","printer":"MK4"}"#,
                    ),
                    "/api/v1/status" => {
                        json_response(200, br#"{"printer":{"state":"PRINTING"}}"#)
                    }
                    _ => Ok(ProbeResponse::new(404, Vec::new())),
                },
                FakeMode::PrusaVersionAuth if request.path == "/api/version" => {
                    Ok(ProbeResponse::new(403, Vec::new()))
                }
                FakeMode::PrusaVersionAuth => Ok(ProbeResponse::new(404, Vec::new())),
                FakeMode::LargeResponse => Ok(ProbeResponse::new(
                    200,
                    vec![b'x'; MAX_RESPONSE_BYTES + 1],
                )),
                FakeMode::CancelAfterFirst(cancelled) => {
                    if first {
                        cancelled.store(true, Ordering::Release);
                    }
                    Ok(ProbeResponse::new(404, Vec::new()))
                }
                FakeMode::Bambu | FakeMode::BambuOutOfScope => {
                    Ok(ProbeResponse::new(404, Vec::new()))
                }
                FakeMode::Elegoo | FakeMode::ElegooUnknown | FakeMode::ElegooNoEvent => Ok(ProbeResponse::new(404, Vec::new())),
            }
        }

        fn bambu_certificate(
            &self,
            address: Ipv4Addr,
            _timeout: Duration,
        ) -> Result<BambuCertificateIdentity, ProbeError> {
            self.bambu_hosts.lock().unwrap().push(address);
            match self.mode.as_ref().unwrap() {
                FakeMode::Bambu => Ok(BambuCertificateIdentity {
                    issuer_common_name: "Bambu Lab Printer CA".into(),
                    issuer_serialized: "CN=Bambu Lab Printer CA,O=BBL".into(),
                    serial_number: Some("AB12CD34EF56".into()),
                }),
                _ => Err(ProbeError::NotBambuCertificate),
            }
        }

        fn elegoo_sdcp_status_refresh(
            &self,
            address: Ipv4Addr,
            mainboard_id: &str,
            timeout: Duration,
        ) -> Result<self::elegoo_sdcp::SdcpStatusObservation, ProbeError> {
            self.elegoo_requests
                .lock()
                .unwrap()
                .push((address, mainboard_id.to_owned(), timeout));
            match self.mode.as_ref().unwrap() {
                FakeMode::Elegoo => Ok(self::elegoo_sdcp::SdcpStatusObservation {
                    event_observed: true,
                    job_status: PrinterJobStatus::Printing,
                }),
                FakeMode::ElegooUnknown | FakeMode::ElegooNoEvent => {
                    Ok(self::elegoo_sdcp::SdcpStatusObservation {
                        event_observed: matches!(self.mode, Some(FakeMode::ElegooUnknown)),
                        job_status: PrinterJobStatus::Unknown,
                    })
                }
                _ => Err(ProbeError::Failed),
            }
        }

        fn elegoo_sdcp_mainboard_id(
            &self,
            address: Ipv4Addr,
            timeout: Duration,
        ) -> Result<String, ProbeError> {
            self.elegoo_discoveries
                .lock()
                .unwrap()
                .push((address, timeout));
            match self.mode.as_ref().unwrap() {
                FakeMode::Elegoo => Ok("AB12CD34EF56".into()),
                _ => Err(ProbeError::Failed),
            }
        }

        fn bambu_ssdp_responses(
            &self,
            scope: &ScanScope,
            _deadline: Instant,
            _request_timeout: Duration,
            _cancelled: &AtomicBool,
        ) -> Vec<(Ipv4Addr, Vec<u8>)> {
            let response = b"HTTP/1.1 200 OK\r\nST: urn:bambulab-com:device:3dprinter:1\r\nUSN: uuid:AB12CD34EF56::urn:bambulab-com:device:3dprinter:1\r\n\r\n".to_vec();
            self.ssdp_requests.fetch_add(1, Ordering::AcqRel);
            match self.mode.as_ref().unwrap() {
                FakeMode::Bambu => scope
                    .addresses()
                    .iter()
                    .copied()
                    .map(|address| (address, response.clone()))
                    .collect(),
                FakeMode::BambuOutOfScope => {
                    vec![(Ipv4Addr::new(192, 168, 4, 250), response)]
                }
                _ => Vec::new(),
            }
        }
    }

    fn json_response(status: u16, body: &[u8]) -> Result<ProbeResponse, ProbeError> {
        Ok(ProbeResponse::new(status, body.to_vec()))
    }

    pub(super) fn fixture_printer(make: Option<&str>, host: &str, approved: bool) -> Printer {
        Printer {
            id: "printer-1".into(),
            metadata_version: 1,
            nickname: "Test printer".into(),
            make: make.map(str::to_owned),
            model: None,
            printer_type: Some("FDM".into()),
            firmware: None,
            host: Some(host.into()),
            web_url: None,
            endpoint_approved: approved,
            bed_mm: None,
            notes: String::new(),
            source_kind: PrinterSourceKind::Manual,
            connection_status: PrinterConnectionStatus::Unknown,
            job_status: PrinterJobStatus::Unknown,
            last_checked_at: None,
            connection_error_code: None,
            print_count: 0,
            last_printed_at: None,
            created_at: "1".into(),
            updated_at: "1".into(),
        }
    }

    #[test]
    fn network_targets_require_private_ipv4_literals_without_ports_or_dns() {
        for host in ["192.168.1.4", "10.0.0.3", "172.20.1.9"] {
            assert!(parse_private_ipv4(host).is_ok(), "rejected {host}");
        }
        for host in [
            "printer.local",
            "8.8.8.8",
            "127.0.0.1",
            "169.254.1.2",
            "192.168.1.4:7125",
            "[::1]",
            "0.0.0.0",
        ] {
            assert!(parse_private_ipv4(host).is_err(), "accepted {host}");
        }
    }

    #[test]
    fn certificate_identity_parser_extracts_fields_without_establishing_trust() {
        let certificate = synthetic_bambu_certificate(
            "Bambu Lab Printer CA",
            "O=BBL,CN=Bambu Lab Printer CA",
            "AB12CD34EF56",
        );
        let identity = parse_bambu_certificate_identity(&certificate).unwrap();
        assert_eq!(identity.serial_number.as_deref(), Some("AB12CD34EF56"));

        let generic_issuer = synthetic_bambu_certificate(
            "Generic Printer CA",
            "O=BBL,CN=Generic Printer CA",
            "AB12CD34EF56",
        );
        assert_eq!(
            parse_bambu_certificate_identity(&generic_issuer)
                .unwrap()
                .issuer_common_name,
            "Generic Printer CA"
        );

        let invalid_serial = synthetic_bambu_certificate(
            "Bambu Lab Printer CA",
            "O=BBL,CN=Bambu Lab Printer CA",
            "serial-too-short",
        );
        assert!(parse_bambu_certificate_identity(&invalid_serial)
            .unwrap()
            .serial_number
            .is_none());
    }

    #[test]
    fn bambu_ca_bundle_is_hash_pinned_and_keeps_cross_signed_cas_out_of_roots() {
        let material = bambu_ca_material().unwrap();

        assert_eq!(material.roots.len(), 3);
        assert_eq!(material.cross_signed_intermediates.len(), 2);
    }

    #[test]
    fn bambu_verifier_requires_a_valid_chain_and_matching_server_name() {
        let root = test_certificate_der(include_bytes!("testdata/bambu-test-root.pem"));
        let leaf = test_certificate_der(include_bytes!("testdata/bambu-test-leaf.pem"));
        let mut roots = rustls::RootCertStore::empty();
        roots.add(root).unwrap();
        let material = BambuCaMaterial {
            roots,
            cross_signed_intermediates: Vec::new(),
        };
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier = BambuCertificateVerifier::new(material, provider).unwrap();
        let matching_name =
            rustls::pki_types::ServerName::IpAddress(Ipv4Addr::new(192, 168, 1, 20).into());
        let verify = |server_name: &rustls::pki_types::ServerName<'_>| {
            <BambuCertificateVerifier as rustls::client::danger::ServerCertVerifier>::verify_server_cert(
                &verifier,
                &leaf,
                &[],
                server_name,
                &[],
                rustls::pki_types::UnixTime::now(),
            )
        };

        assert!(verify(&matching_name).is_ok());
        assert_eq!(
            verifier
                .identity
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .serial_number
                .as_deref(),
            Some("AB12CD34EF56")
        );

        let wrong_name =
            rustls::pki_types::ServerName::IpAddress(Ipv4Addr::new(192, 168, 1, 21).into());
        let wrong_name_verifier = BambuCertificateVerifier::new(
            BambuCaMaterial {
                roots: {
                    let mut roots = rustls::RootCertStore::empty();
                    roots
                        .add(test_certificate_der(include_bytes!(
                            "testdata/bambu-test-root.pem"
                        )))
                        .unwrap();
                    roots
                },
                cross_signed_intermediates: Vec::new(),
            },
            Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
        )
        .unwrap();
        let wrong_name_result =
            <BambuCertificateVerifier as rustls::client::danger::ServerCertVerifier>::verify_server_cert(
                &wrong_name_verifier,
                &leaf,
                &[],
                &wrong_name,
                &[],
                rustls::pki_types::UnixTime::now(),
            );
        assert!(wrong_name_result.is_err());
        assert!(wrong_name_verifier.identity.lock().unwrap().is_none());
    }

    #[test]
    fn bambu_verifier_builds_a_path_with_a_pinned_intermediate() {
        let root = test_certificate_der(include_bytes!("testdata/bambu-chain-root.pem"));
        let intermediate =
            test_certificate_der(include_bytes!("testdata/bambu-cross-intermediate.pem"));
        let leaf = test_certificate_der(include_bytes!("testdata/bambu-cross-leaf.pem"));
        let mut roots = rustls::RootCertStore::empty();
        roots.add(root).unwrap();
        let verifier = BambuCertificateVerifier::new(
            BambuCaMaterial {
                roots,
                cross_signed_intermediates: vec![intermediate],
            },
            Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
        )
        .unwrap();
        let server_name =
            rustls::pki_types::ServerName::IpAddress(Ipv4Addr::new(192, 168, 1, 20).into());

        let result =
            <BambuCertificateVerifier as rustls::client::danger::ServerCertVerifier>::verify_server_cert(
                &verifier,
                &leaf,
                &[],
                &server_name,
                &[],
                rustls::pki_types::UnixTime::now(),
            );

        assert!(result.is_ok());
        assert_eq!(
            verifier
                .identity
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .serial_number
                .as_deref(),
            Some("AB12CD34EF56")
        );
    }

    #[test]
    fn bambu_verifier_rejects_a_certificate_outside_its_validity_window() {
        let root = test_certificate_der(include_bytes!("testdata/bambu-test-root.pem"));
        let leaf = test_certificate_der(include_bytes!("testdata/bambu-test-leaf.pem"));
        let mut roots = rustls::RootCertStore::empty();
        roots.add(root).unwrap();
        let verifier = BambuCertificateVerifier::new(
            BambuCaMaterial {
                roots,
                cross_signed_intermediates: Vec::new(),
            },
            Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
        )
        .unwrap();
        let server_name =
            rustls::pki_types::ServerName::IpAddress(Ipv4Addr::new(192, 168, 1, 20).into());

        let result =
            <BambuCertificateVerifier as rustls::client::danger::ServerCertVerifier>::verify_server_cert(
                &verifier,
                &leaf,
                &[],
                &server_name,
                &[],
                rustls::pki_types::UnixTime::since_unix_epoch(Duration::from_secs(1)),
            );

        assert!(result.is_err());
        assert!(verifier.identity.lock().unwrap().is_none());
    }

    #[test]
    fn bambu_issuer_marker_without_a_trusted_chain_is_rejected() {
        let certificate = synthetic_bambu_certificate(
            "Bambu Lab Printer CA",
            "O=BBL,CN=Bambu Lab Printer CA",
            "AB12CD34EF56",
        );
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier =
            BambuCertificateVerifier::new(bambu_ca_material().unwrap(), provider).unwrap();
        let server_name =
            rustls::pki_types::ServerName::IpAddress(Ipv4Addr::new(192, 168, 1, 20).into());

        let result =
            <BambuCertificateVerifier as rustls::client::danger::ServerCertVerifier>::verify_server_cert(
                &verifier,
                &rustls::pki_types::CertificateDer::from(certificate),
                &[],
                &server_name,
                &[],
                rustls::pki_types::UnixTime::now(),
            );

        assert!(result.is_err(), "issuer text must not establish trust");
        assert!(verifier.identity.lock().unwrap().is_none());
    }

    #[test]
    fn discovery_uses_only_fixed_private_probes_and_stops_at_33_suggestions() {
        let scope = parse_scan_scope("192.168.4.1-192.168.4.40").unwrap();
        let transport = Arc::new(FakeTransport::new(FakeMode::Bambu));
        let suggestions = discover_with_transport(
            &scope,
            transport.clone(),
            Arc::new(AtomicBool::new(false)),
            ScanLimits {
                workers: 4,
                total: Duration::from_secs(2),
                request: Duration::from_millis(250),
                suggestions: 33,
            },
        );

        assert_eq!(suggestions.len(), 33);
        assert!(suggestions.iter().all(|suggestion| {
            suggestion.source == SuggestionSource::BambuMqttTls
                && suggestion.printer_type == "FDM"
                && suggestion.connection.kind == "network"
        }));
        let encoded = serde_json::to_value(&suggestions[0]).unwrap();
        assert!(encoded.get("serialNumber").is_none());
        assert!(!encoded.to_string().contains("AB12CD34EF56"));
        assert_eq!(encoded["source"].as_str(), Some("bambu_mqtt_tls"));
        assert_eq!(encoded["printerType"].as_str(), Some("FDM"));
        assert!(transport.requests.lock().unwrap().is_empty());
        let bambu_probes = transport.bambu_hosts.lock().unwrap().len();
        assert!((33..=36).contains(&bambu_probes));
        assert!(transport
            .maximum_timeout
            .lock()
            .unwrap()
            .le(&Duration::from_secs(2)));
    }

    #[test]
    fn discovery_finds_moonraker_and_prusalink_from_synthetic_responses() {
        let scope = parse_scan_scope("192.168.50.20/32").unwrap();
        let limits = ScanLimits {
            workers: 1,
            total: Duration::from_secs(2),
            request: Duration::from_millis(250),
            suggestions: 33,
        };
        let moonraker = discover_with_transport(
            &scope,
            Arc::new(FakeTransport::new(FakeMode::Moonraker)),
            Arc::new(AtomicBool::new(false)),
            limits,
        );
        assert_eq!(moonraker.len(), 1);
        assert_eq!(moonraker[0].source, SuggestionSource::Moonraker);
        assert_eq!(moonraker[0].connection.address, "192.168.50.20");

        let prusalink_transport = Arc::new(FakeTransport::new(FakeMode::PrusaLink));
        let prusalink = discover_with_transport(
            &scope,
            prusalink_transport.clone(),
            Arc::new(AtomicBool::new(false)),
            limits,
        );
        assert_eq!(prusalink.len(), 1);
        assert_eq!(prusalink[0].source, SuggestionSource::Prusalink);
        assert_eq!(prusalink[0].name, "MK4");
        assert!(prusalink_transport
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.path == "/api/version" && request.port == 443));
    }

    #[test]
    fn discovery_discards_out_of_scope_ssdp_replies_before_bambu_tls() {
        let scope = parse_scan_scope("192.168.5.10/32").unwrap();
        let transport = Arc::new(FakeTransport::new(FakeMode::BambuOutOfScope));
        let suggestions = discover_with_transport(
            &scope,
            transport.clone(),
            Arc::new(AtomicBool::new(false)),
            ScanLimits {
                workers: 1,
                total: Duration::from_secs(2),
                request: Duration::from_millis(100),
                suggestions: 33,
            },
        );

        assert!(suggestions
            .iter()
            .all(|suggestion| suggestion.source != SuggestionSource::BambuMqttTls));
        assert!(transport.bambu_hosts.lock().unwrap().is_empty());
    }

    #[test]
    fn prusalink_successful_status_fixture_reports_printing() {
        let printer = fixture_printer(Some("Prusa Research"), "192.168.1.20", true);
        let transport = FakeTransport::new(FakeMode::PrusaLink);
        let outcome = check_printer_with_transport(
            &printer,
            &transport,
            Instant::now() + Duration::from_secs(2),
            Duration::from_millis(250),
        )
        .unwrap();

        assert_eq!(outcome.result, PrinterConnectionTestResult::Connected);
        assert_eq!(outcome.connection_status, PrinterConnectionStatus::Online);
        assert_eq!(outcome.job_status, PrinterJobStatus::Printing);
        assert!(transport
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.path == "/api/v1/status" && request.port == 443));
    }

    #[test]
    fn discovery_observes_cancellation_between_fixed_requests() {
        let scope = parse_scan_scope("10.0.0.1-10.0.0.5").unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let transport = Arc::new(FakeTransport::new(FakeMode::CancelAfterFirst(
            cancelled.clone(),
        )));
        let suggestions = discover_with_transport(
            &scope,
            transport.clone(),
            cancelled,
            ScanLimits {
                workers: 1,
                total: Duration::from_secs(2),
                request: Duration::from_millis(250),
                suggestions: 33,
            },
        );
        assert!(suggestions.is_empty());
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn prusalink_auth_required_is_returned_without_any_credential_input() {
        let printer = fixture_printer(Some("Prusa Research"), "192.168.1.20", true);
        let transport = FakeTransport::new(FakeMode::PrusaAuth);
        let outcome = check_printer_with_transport(
            &printer,
            &transport,
            Instant::now() + Duration::from_secs(2),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(outcome.result, PrinterConnectionTestResult::AuthRequired);
        assert_eq!(
            outcome.connection_status,
            PrinterConnectionStatus::AuthRequired
        );
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("credentials_unavailable")
        );
        assert!(transport
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| { request.path == "/api/v1/status" && request.port == 443 }));
        assert_eq!(
            serde_json::to_value(PrinterConnectionTestResult::AuthRequired).unwrap(),
            serde_json::json!("authRequired")
        );
    }

    #[test]
    fn moonraker_status_is_separate_from_connectivity() {
        let printer = fixture_printer(Some("Klipper"), "10.0.0.22", true);
        let transport = FakeTransport::new(FakeMode::Moonraker);
        let outcome = check_printer_with_transport(
            &printer,
            &transport,
            Instant::now() + Duration::from_secs(2),
            Duration::from_millis(250),
        )
        .unwrap();

        assert_eq!(outcome.result, PrinterConnectionTestResult::Connected);
        assert_eq!(outcome.connection_status, PrinterConnectionStatus::Online);
        assert_eq!(outcome.job_status, PrinterJobStatus::Printing);
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests[0].path, "/server/info");
        assert_eq!(requests[1].path, "/printer/info");
        assert_eq!(
            requests[2].path,
            "/printer/objects/query?print_stats&webhooks"
        );
    }

    #[test]
    fn moonraker_auth_responses_do_not_fall_through_to_offline() {
        for path in [
            "/server/info",
            "/printer/info",
            "/printer/objects/query?print_stats&webhooks",
        ] {
            let printer = fixture_printer(Some("Klipper"), "10.0.0.22", true);
            let transport = FakeTransport::new(FakeMode::MoonrakerAuth(path));
            let outcome = check_printer_with_transport(
                &printer,
                &transport,
                Instant::now() + Duration::from_secs(2),
                Duration::from_millis(250),
            )
            .unwrap();

            assert_eq!(
                outcome.result,
                PrinterConnectionTestResult::AuthRequired,
                "{path}"
            );
            assert_eq!(
                outcome.connection_status,
                PrinterConnectionStatus::AuthRequired
            );
            assert_eq!(outcome.job_status, PrinterJobStatus::Unknown);
        }
    }

    #[test]
    fn prusalink_version_auth_response_is_reported_without_credentials() {
        let printer = fixture_printer(Some("Prusa Research"), "192.168.1.20", true);
        let transport = FakeTransport::new(FakeMode::PrusaVersionAuth);
        let outcome = check_printer_with_transport(
            &printer,
            &transport,
            Instant::now() + Duration::from_secs(2),
            Duration::from_millis(250),
        )
        .unwrap();

        assert_eq!(outcome.result, PrinterConnectionTestResult::AuthRequired);
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("credentials_unavailable")
        );
        assert!(transport
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.path != "/api/v1/status"));
    }

    #[test]
    fn prusalink_detection_requires_a_prusa_specific_body_or_header_signal() {
        let generic = serde_json::json!({"api": "2.0", "version": "1"});
        assert!(!is_prusalink_version(&generic, &[]));

        let identified_body = serde_json::json!({"firmware": "PrusaLink 0.7.2"});
        assert!(is_prusalink_version(&identified_body, &[]));

        assert!(is_prusalink_version(&generic, &["PrusaLink".to_owned()]));
        assert!(!is_prusalink_version(
            &generic,
            &["Generic server".to_owned()]
        ));
        assert!(!is_prusalink_version(
            &generic,
            &[format!("Prusa{}", "x".repeat(128))]
        ));
    }

    #[test]
    fn requests_reject_dynamic_paths_and_bodies_over_the_cap() {
        let dynamic = ProbeRequest {
            address: Ipv4Addr::new(192, 168, 1, 4),
            scheme: Scheme::Http,
            port: MOONRAKER_PORT,
            path: "/printer/objects/query?user_supplied=1",
        };
        assert!(!fixed_http_path(&dynamic));

        let scope = parse_scan_scope("192.168.1.4/32").unwrap();
        let transport = FakeTransport::new(FakeMode::LargeResponse);
        let result = safe_get(
            &transport,
            scope.addresses()[0],
            Scheme::Http,
            MOONRAKER_PORT,
            "/server/info",
            Instant::now() + Duration::from_secs(1),
            Duration::from_millis(250),
            None,
        );
        assert_eq!(result.unwrap_err(), ProbeError::TooLarge);
    }

    #[test]
    fn bambu_selected_host_returns_auth_required_after_cert_check_without_mqtt() {
        let printer = fixture_printer(Some("Bambu Lab"), "192.168.1.30", true);
        let transport = FakeTransport::new(FakeMode::Bambu);
        let outcome = check_printer_with_transport(
            &printer,
            &transport,
            Instant::now() + Duration::from_secs(2),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(outcome.result, PrinterConnectionTestResult::AuthRequired);
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("credentials_unavailable")
        );
        assert_eq!(
            transport.bambu_hosts.lock().unwrap().as_slice(),
            &[Ipv4Addr::new(192, 168, 1, 30)]
        );
        assert!(transport.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn original_centauri_carbon_uses_fixed_sdcp_refresh_and_maps_observed_printing() {
        let mut printer = fixture_printer(Some("Elegoo"), "192.168.0.108", true);
        printer.model = Some("Centauri Carbon".into());
        let transport = FakeTransport::new(FakeMode::Elegoo);
        let outcome = check_printer_with_transport_and_sdcp(
            &printer,
            &transport,
            Instant::now() + Duration::from_secs(2),
            Duration::from_secs(2),
            Some("AB12CD34EF56"),
            false,
        )
        .unwrap();

        assert_eq!(outcome.result, PrinterConnectionTestResult::Connected);
        assert_eq!(outcome.connection_status, PrinterConnectionStatus::Online);
        assert_eq!(outcome.job_status, PrinterJobStatus::Printing);
        assert!(outcome.status_event_observed);
        let requests = transport.elegoo_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, Ipv4Addr::new(192, 168, 0, 108));
        assert_eq!(requests[0].1, "AB12CD34EF56");
        assert!(requests[0].2 <= Duration::from_secs(2));
        assert!(transport.requests.lock().unwrap().is_empty());
        assert!(transport.bambu_hosts.lock().unwrap().is_empty());
        assert!(transport.elegoo_discoveries.lock().unwrap().is_empty());
    }

    #[test]
    fn original_centauri_carbon_keeps_unknown_for_unmapped_or_absent_status() {
        for (mode, event_observed) in [
            (FakeMode::ElegooUnknown, true),
            (FakeMode::ElegooNoEvent, false),
        ] {
            let mut printer = fixture_printer(Some("Elegoo"), "192.168.0.108", true);
            printer.model = Some("Centauri Carbon".into());
            let transport = FakeTransport::new(mode);
            let outcome = check_printer_with_transport_and_sdcp(
                &printer,
                &transport,
                Instant::now() + Duration::from_secs(2),
                Duration::from_secs(2),
                Some("AB12CD34EF56"),
                false,
            )
            .unwrap();
            assert_eq!(outcome.result, PrinterConnectionTestResult::Connected);
            assert_eq!(outcome.job_status, PrinterJobStatus::Unknown);
            assert_eq!(outcome.status_event_observed, event_observed);
        }
    }

    #[test]
    fn explicit_sdcp_discovery_finds_id_then_sends_one_status_refresh_to_the_saved_target() {
        let mut printer = fixture_printer(Some("Elegoo"), "192.168.0.108", true);
        printer.model = Some("Centauri Carbon".into());
        let transport = FakeTransport::new(FakeMode::Elegoo);
        let outcome = check_printer_with_transport_and_sdcp(
            &printer,
            &transport,
            Instant::now() + MAX_ELEGOO_DISCOVERY_TEST_DURATION,
            MAX_REQUEST_DURATION,
            None,
            true,
        )
        .unwrap();

        assert_eq!(outcome.result, PrinterConnectionTestResult::Connected);
        assert_eq!(outcome.job_status, PrinterJobStatus::Printing);
        assert!(outcome.status_event_observed);
        assert_eq!(
            transport
                .elegoo_discoveries
                .lock()
                .unwrap()
                .as_slice()
                .len(),
            1
        );
        let discoveries = transport.elegoo_discoveries.lock().unwrap();
        assert_eq!(discoveries[0].0, Ipv4Addr::new(192, 168, 0, 108));
        assert!(discoveries[0].1 <= MAX_REQUEST_DURATION);
        assert_eq!(
            transport.elegoo_requests.lock().unwrap()[0].1,
            "AB12CD34EF56"
        );
    }

    #[test]
    fn original_centauri_carbon_requires_a_transient_mainboard_id() {
        let mut printer = fixture_printer(Some("Elegoo"), "192.168.0.108", true);
        printer.model = Some("Centauri Carbon".into());
        let transport = FakeTransport::new(FakeMode::Elegoo);
        assert_eq!(
            check_printer_with_transport_and_sdcp(
                &printer,
                &transport,
                Instant::now() + Duration::from_secs(2),
                Duration::from_secs(2),
                None,
                false,
            )
            .unwrap()
            .error_code,
            Some("mainboard_id_required")
        );
        assert!(transport.elegoo_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn sdcp_discovery_refuses_unapproved_and_unsupported_printers_before_transport() {
        let transport = FakeTransport::new(FakeMode::Elegoo);
        let generic = fixture_printer(Some("Elegoo"), "192.168.0.108", true);
        assert_eq!(
            check_printer_with_transport_and_sdcp(
                &generic,
                &transport,
                Instant::now() + MAX_ELEGOO_DISCOVERY_TEST_DURATION,
                MAX_REQUEST_DURATION,
                None,
                true,
            )
            .unwrap_err(),
            ProbeError::InvalidResponse
        );

        let mut original = fixture_printer(Some("Elegoo"), "192.168.0.108", false);
        original.model = Some("Centauri Carbon".into());
        assert_eq!(
            check_printer_with_transport_and_sdcp(
                &original,
                &transport,
                Instant::now() + MAX_ELEGOO_DISCOVERY_TEST_DURATION,
                MAX_REQUEST_DURATION,
                None,
                true,
            )
            .unwrap_err(),
            ProbeError::InvalidResponse
        );
        assert!(transport.elegoo_discoveries.lock().unwrap().is_empty());
        assert!(transport.elegoo_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn discovery_confirmation_recheck_requires_the_exact_saved_printer_snapshot() {
        let mut saved = fixture_printer(Some("Elegoo"), "192.168.0.108", true);
        saved.model = Some("Centauri Carbon".into());
        assert!(same_printer_snapshot(&saved, Some(&saved)));
        assert!(!same_printer_snapshot(&saved, None));

        let mut changed_endpoint = saved.clone();
        changed_endpoint.host = Some("192.168.0.109".into());
        assert!(!same_printer_snapshot(&saved, Some(&changed_endpoint)));

        let mut revoked_approval = saved.clone();
        revoked_approval.endpoint_approved = false;
        assert!(!same_printer_snapshot(&saved, Some(&revoked_approval)));
    }

    #[test]
    fn unapproved_or_non_private_saved_target_is_rejected_before_transport() {
        let transport = FakeTransport::new(FakeMode::Bambu);
        let unapproved = fixture_printer(Some("Bambu Lab"), "192.168.1.30", false);
        assert!(check_printer_with_transport(
            &unapproved,
            &transport,
            Instant::now() + Duration::from_secs(2),
            Duration::from_secs(2),
        )
        .is_err());

        let public = fixture_printer(Some("Bambu Lab"), "8.8.8.8", true);
        assert!(check_printer_with_transport(
            &public,
            &transport,
            Instant::now() + Duration::from_secs(2),
            Duration::from_secs(2),
        )
        .is_err());
        assert!(transport.requests.lock().unwrap().is_empty());
        assert!(transport.bambu_hosts.lock().unwrap().is_empty());
    }

    fn der(tag: u8, value: &[u8]) -> Vec<u8> {
        let mut encoded = vec![tag];
        if value.len() < 128 {
            encoded.push(value.len() as u8);
        } else {
            let bytes = value.len().to_be_bytes();
            let first = bytes
                .iter()
                .position(|byte| *byte != 0)
                .unwrap_or(bytes.len() - 1);
            let length = &bytes[first..];
            encoded.push(0x80 | length.len() as u8);
            encoded.extend_from_slice(length);
        }
        encoded.extend_from_slice(value);
        encoded
    }

    fn name(cn: &str, organization: &str) -> Vec<u8> {
        let mut common_name = der(0x06, &[0x55, 0x04, 0x03]);
        common_name.extend(der(0x0c, cn.as_bytes()));
        let mut org_name = der(0x06, &[0x55, 0x04, 0x0a]);
        org_name.extend(der(0x0c, organization.as_bytes()));
        let rdn_cn = der(0x31, &der(0x30, &common_name));
        let rdn_org = der(0x31, &der(0x30, &org_name));
        let mut contents = rdn_cn;
        contents.extend(rdn_org);
        der(0x30, &contents)
    }

    fn test_certificate_der(pem: &[u8]) -> rustls::pki_types::CertificateDer<'static> {
        let text = std::str::from_utf8(pem).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("-----BEGIN CERTIFICATE-----"));
        let mut encoded = String::new();
        for line in lines.by_ref() {
            if line == "-----END CERTIFICATE-----" {
                break;
            }
            encoded.push_str(line);
        }
        assert_eq!(lines.next(), None);
        rustls::pki_types::CertificateDer::from(
            base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                encoded.as_bytes(),
            )
            .unwrap(),
        )
    }

    fn synthetic_bambu_certificate(issuer_cn: &str, org: &str, subject_cn: &str) -> Vec<u8> {
        let mut tbs = der(0xa0, &der(0x02, &[2]));
        tbs.extend(der(0x02, &[1]));
        tbs.extend(der(0x30, &[]));
        tbs.extend(name(issuer_cn, org));
        tbs.extend(der(0x30, &[]));
        tbs.extend(name(subject_cn, "Printer"));
        tbs.extend(der(0x30, &[]));
        let mut certificate = der(0x30, &tbs);
        certificate.extend(der(0x30, &[]));
        certificate.extend(der(0x03, &[0]));
        der(0x30, &certificate)
    }
    #[test]
    fn unicast_only_discovery_never_calls_the_ssdp_transport() {
        let transport = Arc::new(FakeTransport::new(FakeMode::Moonraker));
        let scope = parse_scan_scope("192.168.200.10").unwrap();
        let suggestions = discover_with_transport_options(
            &scope,
            transport.clone(),
            Arc::new(AtomicBool::new(false)),
            ScanLimits::default(),
            false,
        );
        assert_eq!(suggestions.len(), 1);
        assert_eq!(transport.ssdp_requests.load(Ordering::Acquire), 0);
        assert!(transport.bambu_hosts.lock().unwrap().is_empty());
        let _ = discover_with_transport_options(
            &scope,
            transport.clone(),
            Arc::new(AtomicBool::new(false)),
            ScanLimits::default(),
            true,
        );
        assert_eq!(transport.ssdp_requests.load(Ordering::Acquire), 1);
    }
}
use std::collections::BTreeSet;
use std::fmt;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use library_core::models::{Printer, PrinterConnectionStatus, PrinterJobStatus};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub use self::scope::parse_scan_scope;
use self::scope::ScanScope;

const MAX_SCAN_WORKERS: usize = 32;
const MAX_SCAN_SUGGESTIONS: usize = 33;
const MAX_SCAN_DURATION: Duration = Duration::from_secs(30);
const MAX_REQUEST_DURATION: Duration = Duration::from_secs(2);
const MAX_ELEGOO_DISCOVERY_TEST_DURATION: Duration = Duration::from_secs(4);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MOONRAKER_PORT: u16 = 7125;
const BAMBU_MQTT_TLS_PORT: u16 = 8883;
const BAMBU_CERTIFICATE_BUNDLE_SHA256: &str =
    "36f2bcee347ec7adce719b5fd350099591a4d3d0ec4e039c7019890d78e152a0";
const BAMBU_ROOT_CERTIFICATE_SHA256: [&str; 3] = [
    "e98f19578b3f124ace6b8a247ffeda52dc99c89fd4e7d20c82829977b7f33502",
    "1f99d846718c8d333ac319cb8e70919ab63236090fbadde909ba4a6f0867a683",
    "030bca81cece18b7eff3cfd2b75d09d3efca893bc069609e37fa04257fe4d840",
];
const BAMBU_CROSS_SIGNED_CERTIFICATE_SHA256: [&str; 2] = [
    "3d13019b45a4fa8a6cb8dc0fd7becbb19bb2bf0c2fcb9434a9345fa58680da0b",
    "e898483224f9092f575d627433bb21ed9d125a77e3635b0da6fc18fe850a02d7",
];
const BAMBU_CERTIFICATE_COUNT: usize = 5;

/// Network-only suggestion returned by a user-started discovery scan.
/// It deliberately contains no device serials, credentials, or persisted state.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub key: String,
    pub name: String,
    pub model: Option<String>,
    pub bed_mm: Option<[u32; 3]>,
    pub connection: NetworkConnection,
    pub source: SuggestionSource,
    pub printer_type: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkConnection {
    pub kind: String,
    pub address: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionSource {
    Moonraker,
    Prusalink,
    BambuMqttTls,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PrinterConnectionTestResult {
    Connected,
    Unreachable,
    Unsupported,
    AuthRequired,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterConnectionTestResponse {
    pub result: PrinterConnectionTestResult,
    pub printer: Printer,
    pub status_event_observed: bool,
}

#[derive(Clone, Copy)]
struct ScanLimits {
    workers: usize,
    total: Duration,
    request: Duration,
    suggestions: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            workers: MAX_SCAN_WORKERS,
            total: MAX_SCAN_DURATION,
            request: MAX_REQUEST_DURATION,
            suggestions: MAX_SCAN_SUGGESTIONS,
        }
    }
}

impl ScanLimits {
    fn bounded(self) -> Self {
        Self {
            workers: self.workers.clamp(1, MAX_SCAN_WORKERS),
            total: self.total.min(MAX_SCAN_DURATION),
            request: self.request.min(MAX_REQUEST_DURATION),
            suggestions: self.suggestions.min(MAX_SCAN_SUGGESTIONS),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scheme {
    Http,
    Https,
}

#[derive(Debug, Clone)]
struct ProbeRequest {
    address: Ipv4Addr,
    scheme: Scheme,
    port: u16,
    path: &'static str,
}

#[derive(Debug, Clone)]
struct ProbeResponse {
    status: u16,
    body: Vec<u8>,
    /// Bounded Prusa-identification headers; never returned or persisted.
    prusa_header_hints: Vec<String>,
}

impl ProbeResponse {
    fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            body,
            prusa_header_hints: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeError {
    Timeout,
    Unreachable,
    TooLarge,
    Redirect,
    InvalidResponse,
    AcknowledgementMissing,
    NotBambuCertificate,
    Failed,
}

struct BambuCaMaterial {
    roots: rustls::RootCertStore,
    cross_signed_intermediates: Vec<rustls::pki_types::CertificateDer<'static>>,
}

fn bambu_ca_material() -> Result<BambuCaMaterial, ProbeError> {
    let bundle = include_bytes!("../../resources/bambu/printer.cer");
    if bundle.len() != 6_540
        || format!("{:x}", Sha256::digest(bundle)) != BAMBU_CERTIFICATE_BUNDLE_SHA256
    {
        return Err(ProbeError::Failed);
    }

    let text = std::str::from_utf8(bundle).map_err(|_| ProbeError::Failed)?;
    let mut lines = text.lines();
    let mut certificates = Vec::with_capacity(BAMBU_CERTIFICATE_COUNT);
    while let Some(line) = lines.next() {
        if line.trim().is_empty() {
            continue;
        }
        if line != "-----BEGIN CERTIFICATE-----" {
            return Err(ProbeError::Failed);
        }

        let mut encoded = String::new();
        let mut ended = false;
        for line in lines.by_ref() {
            if line == "-----END CERTIFICATE-----" {
                ended = true;
                break;
            }
            if encoded.len().saturating_add(line.len()) > 16_384
                || !line
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
            {
                return Err(ProbeError::Failed);
            }
            encoded.push_str(line);
        }
        if !ended {
            return Err(ProbeError::Failed);
        }
        let der = base64::engine::general_purpose::STANDARD
            .decode(encoded.as_bytes())
            .map_err(|_| ProbeError::Failed)?;
        certificates.push(rustls::pki_types::CertificateDer::from(der));
        if certificates.len() > BAMBU_CERTIFICATE_COUNT {
            return Err(ProbeError::Failed);
        }
    }

    if certificates.len() != BAMBU_CERTIFICATE_COUNT {
        return Err(ProbeError::Failed);
    }

    let mut roots = rustls::RootCertStore::empty();
    let mut cross_signed_intermediates = Vec::with_capacity(2);
    let mut seen_fingerprints = BTreeSet::new();
    for certificate in certificates {
        let fingerprint = format!("{:x}", Sha256::digest(certificate.as_ref()));
        if !seen_fingerprints.insert(fingerprint.clone()) {
            return Err(ProbeError::Failed);
        }
        if BAMBU_ROOT_CERTIFICATE_SHA256.contains(&fingerprint.as_str()) {
            roots.add(certificate).map_err(|_| ProbeError::Failed)?;
        } else if BAMBU_CROSS_SIGNED_CERTIFICATE_SHA256.contains(&fingerprint.as_str()) {
            cross_signed_intermediates.push(certificate);
        } else {
            return Err(ProbeError::Failed);
        }
    }

    if roots.len() != BAMBU_ROOT_CERTIFICATE_SHA256.len()
        || cross_signed_intermediates.len() != BAMBU_CROSS_SIGNED_CERTIFICATE_SHA256.len()
    {
        return Err(ProbeError::Failed);
    }

    Ok(BambuCaMaterial {
        roots,
        cross_signed_intermediates,
    })
}

#[derive(Clone)]
struct BambuCertificateIdentity {
    issuer_common_name: String,
    issuer_serialized: String,
    serial_number: Option<String>,
}

impl fmt::Debug for BambuCertificateIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BambuCertificateIdentity")
            .field("issuer_common_name", &self.issuer_common_name)
            .field("issuer_serialized", &self.issuer_serialized)
            .field(
                "serial_number",
                &self.serial_number.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

trait ProbeTransport: Send + Sync {
    fn get(&self, request: ProbeRequest, timeout: Duration) -> Result<ProbeResponse, ProbeError>;
    fn bambu_ssdp_responses(
        &self,
        _scope: &ScanScope,
        _deadline: Instant,
        _request_timeout: Duration,
        _cancelled: &AtomicBool,
    ) -> Vec<(Ipv4Addr, Vec<u8>)> {
        Vec::new()
    }
    fn bambu_certificate(
        &self,
        address: Ipv4Addr,
        timeout: Duration,
    ) -> Result<BambuCertificateIdentity, ProbeError>;
    fn elegoo_sdcp_status_refresh(
        &self,
        _address: Ipv4Addr,
        _mainboard_id: &str,
        _timeout: Duration,
    ) -> Result<self::elegoo_sdcp::SdcpStatusObservation, ProbeError> {
        Err(ProbeError::Failed)
    }
    fn elegoo_sdcp_mainboard_id(
        &self,
        _address: Ipv4Addr,
        _timeout: Duration,
    ) -> Result<String, ProbeError> {
        Err(ProbeError::Failed)
    }
}

struct NativeTransport {
    guard: NetworkGuard,
    http: reqwest::blocking::Client,
}

impl NativeTransport {
    fn new(guard: NetworkGuard) -> Result<Self, ProbeError> {
        if !guard.permitted() {
            return Err(ProbeError::Failed);
        }
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(MAX_REQUEST_DURATION)
            .build()
            .map_err(|_| ProbeError::Failed)?;
        Ok(Self { http, guard })
    }
}

impl ProbeTransport for NativeTransport {
    fn get(&self, request: ProbeRequest, timeout: Duration) -> Result<ProbeResponse, ProbeError> {
        if !self.guard.permitted() {
            return Err(ProbeError::Failed);
        }
        if !fixed_http_path(&request) {
            return Err(ProbeError::InvalidResponse);
        }
        let scheme = match request.scheme {
            Scheme::Http => "http",
            Scheme::Https => "https",
        };
        let url = format!(
            "{scheme}://{}:{}/{}",
            request.address,
            request.port,
            request.path.trim_start_matches('/')
        );
        let mut response = self
            .http
            .get(url)
            .timeout(timeout.min(MAX_REQUEST_DURATION))
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .map_err(classify_reqwest_error)?;
        let status = response.status();
        if status.is_redirection() {
            return Err(ProbeError::Redirect);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(ProbeError::TooLarge);
        }
        let mut body = Vec::new();
        response
            .by_ref()
            .take((MAX_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut body)
            .map_err(classify_io_error)?;
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(ProbeError::TooLarge);
        }
        let prusa_header_hints = [reqwest::header::SERVER, reqwest::header::WWW_AUTHENTICATE]
            .into_iter()
            .filter_map(|name| response.headers().get(name))
            .filter_map(|value| value.to_str().ok())
            .filter(|value| value.len() <= 128 && !value.chars().any(char::is_control))
            .map(str::to_owned)
            .collect();
        let mut probe_response = ProbeResponse::new(status.as_u16(), body);
        probe_response.prusa_header_hints = prusa_header_hints;
        Ok(probe_response)
    }

    fn bambu_certificate(
        &self,
        address: Ipv4Addr,
        timeout: Duration,
    ) -> Result<BambuCertificateIdentity, ProbeError> {
        bambu_tls_certificate(address, timeout.min(MAX_REQUEST_DURATION), &self.guard)
    }

    fn elegoo_sdcp_status_refresh(
        &self,
        address: Ipv4Addr,
        mainboard_id: &str,
        timeout: Duration,
    ) -> Result<self::elegoo_sdcp::SdcpStatusObservation, ProbeError> {
        self::elegoo_sdcp::send_status_refresh(
            address,
            mainboard_id,
            timeout.min(MAX_REQUEST_DURATION),
            &self.guard,
        )
        .map_err(|error| match error {
            self::elegoo_sdcp::SdcpRequestError::Timeout => ProbeError::Timeout,
            self::elegoo_sdcp::SdcpRequestError::Unreachable => ProbeError::Unreachable,
            self::elegoo_sdcp::SdcpRequestError::InvalidMainboardId => ProbeError::InvalidResponse,
            self::elegoo_sdcp::SdcpRequestError::AcknowledgementMissing => {
                ProbeError::AcknowledgementMissing
            }
            _ => ProbeError::Failed,
        })
    }

    fn elegoo_sdcp_mainboard_id(
        &self,
        address: Ipv4Addr,
        timeout: Duration,
    ) -> Result<String, ProbeError> {
        self::elegoo_discovery::discover_mainboard_id(
            address,
            timeout.min(MAX_REQUEST_DURATION),
            &self.guard,
        )
        .map_err(|error| match error {
            self::elegoo_discovery::DiscoveryError::Timeout => ProbeError::Timeout,
            self::elegoo_discovery::DiscoveryError::Unreachable => ProbeError::Unreachable,
            self::elegoo_discovery::DiscoveryError::InvalidResponse => ProbeError::InvalidResponse,
            self::elegoo_discovery::DiscoveryError::Failed => ProbeError::Failed,
        })
    }

    fn bambu_ssdp_responses(
        &self,
        scope: &ScanScope,
        deadline: Instant,
        request_timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Vec<(Ipv4Addr, Vec<u8>)> {
        self::bambu_discovery::search(scope, deadline, request_timeout, cancelled, &self.guard)
    }
}

fn classify_reqwest_error(error: reqwest::Error) -> ProbeError {
    if error.is_timeout() {
        ProbeError::Timeout
    } else if error.is_connect() {
        ProbeError::Unreachable
    } else {
        ProbeError::Failed
    }
}

fn classify_io_error(error: std::io::Error) -> ProbeError {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ProbeError::Timeout,
        std::io::ErrorKind::InvalidData => ProbeError::NotBambuCertificate,
        std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::NotConnected
        | std::io::ErrorKind::AddrNotAvailable => ProbeError::Unreachable,
        _ => ProbeError::Failed,
    }
}

fn fixed_http_path(request: &ProbeRequest) -> bool {
    match (request.scheme, request.port, request.path) {
        (Scheme::Http, MOONRAKER_PORT, "/server/info")
        | (Scheme::Http, MOONRAKER_PORT, "/printer/info")
        | (Scheme::Http, MOONRAKER_PORT, "/printer/objects/query?print_stats&webhooks")
        | (Scheme::Http, 80, "/server/info")
        | (Scheme::Http, 80, "/printer/info")
        | (Scheme::Http, 80, "/printer/objects/query?print_stats&webhooks")
        | (Scheme::Http, 80, "/api/version")
        | (Scheme::Http, 80, "/api/v1/status")
        | (Scheme::Https, 443, "/api/version")
        | (Scheme::Https, 443, "/api/v1/status") => true,
        _ => false,
    }
}

fn parse_private_ipv4(value: &str) -> Result<Ipv4Addr, ProbeError> {
    let address = value
        .parse::<Ipv4Addr>()
        .map_err(|_| ProbeError::InvalidResponse)?;
    if address.to_string() != value
        || !address.is_private()
        || address.is_loopback()
        || address.is_link_local()
        || address.is_multicast()
        || address.is_unspecified()
        || address == Ipv4Addr::BROADCAST
    {
        return Err(ProbeError::InvalidResponse);
    }
    Ok(address)
}

fn parse_printer_address(printer: &Printer) -> Result<Ipv4Addr, ProbeError> {
    if let Some(host) = printer.host.as_deref() {
        return parse_private_ipv4(host);
    }
    let web_url = printer
        .web_url
        .as_deref()
        .ok_or(ProbeError::InvalidResponse)?;
    let (scheme, rest) = if let Some(rest) = web_url.strip_prefix("https://") {
        (Scheme::Https, rest)
    } else if let Some(rest) = web_url.strip_prefix("http://") {
        (Scheme::Http, rest)
    } else {
        return Err(ProbeError::InvalidResponse);
    };
    let _ = scheme;
    if rest
        .chars()
        .any(|character| matches!(character, '@' | '?' | '#' | ':'))
    {
        return Err(ProbeError::InvalidResponse);
    }
    let authority = rest.split('/').next().unwrap_or_default();
    parse_private_ipv4(authority)
}

fn safe_get(
    transport: &dyn ProbeTransport,
    address: Ipv4Addr,
    scheme: Scheme,
    port: u16,
    path: &'static str,
    deadline: Instant,
    request_limit: Duration,
    cancelled: Option<&AtomicBool>,
) -> Result<ProbeResponse, ProbeError> {
    if cancelled.is_some_and(|value| value.load(Ordering::Acquire)) {
        return Err(ProbeError::Failed);
    }
    let timeout = bounded_timeout(deadline, request_limit)?;
    let request = ProbeRequest {
        address,
        scheme,
        port,
        path,
    };
    if !fixed_http_path(&request) {
        return Err(ProbeError::InvalidResponse);
    }
    let response = transport.get(request, timeout)?;
    if response.body.len() > MAX_RESPONSE_BYTES {
        return Err(ProbeError::TooLarge);
    }
    Ok(response)
}

fn bounded_timeout(deadline: Instant, request_limit: Duration) -> Result<Duration, ProbeError> {
    let timeout = deadline
        .saturating_duration_since(Instant::now())
        .min(request_limit)
        .min(MAX_REQUEST_DURATION);
    if timeout.is_zero() {
        Err(ProbeError::Timeout)
    } else {
        Ok(timeout)
    }
}

fn probe_bambu_certificate(
    transport: &dyn ProbeTransport,
    address: Ipv4Addr,
    deadline: Instant,
    request_limit: Duration,
) -> Result<BambuCertificateIdentity, ProbeError> {
    transport.bambu_certificate(address, bounded_timeout(deadline, request_limit)?)
}

fn success_json(response: &ProbeResponse) -> Option<Value> {
    (200..300)
        .contains(&response.status)
        .then(|| serde_json::from_slice::<Value>(&response.body).ok())
        .flatten()
}

fn auth_response(response: &ProbeResponse) -> bool {
    response.status == 401 || response.status == 403
}

fn bounded_string(value: Option<&Value>, limit: usize) -> Option<String> {
    let text = value?.as_str()?.trim();
    if text.is_empty() || text.len() > limit || text.chars().any(char::is_control) {
        return None;
    }
    Some(text.to_owned())
}

fn is_moonraker_info(value: &Value) -> bool {
    value
        .get("result")
        .and_then(Value::as_object)
        .is_some_and(|result| {
            result.contains_key("klippy_state") || result.contains_key("moonraker_version")
        })
}

fn is_prusalink_version(value: &Value, headers: &[String]) -> bool {
    let body_identifies_prusa = ["text", "hostname", "server", "firmware"]
        .iter()
        .filter_map(|field| bounded_string(value.get(*field), 128))
        .any(|text| text.to_ascii_lowercase().contains("prusa"));
    let header_identifies_prusa = headers.iter().any(|header| {
        header.len() <= 128
            && !header.chars().any(char::is_control)
            && header.to_ascii_lowercase().contains("prusa")
    });
    body_identifies_prusa || header_identifies_prusa
}

fn make_suggestion(
    address: Ipv4Addr,
    source: SuggestionSource,
    name: String,
    model: Option<String>,
) -> Suggestion {
    let source_key = match source {
        SuggestionSource::Moonraker => "moonraker",
        SuggestionSource::Prusalink => "prusalink",
        SuggestionSource::BambuMqttTls => "bambu_mqtt_tls",
    };
    Suggestion {
        key: format!("{source_key}:{address}"),
        name,
        model,
        bed_mm: None,
        connection: NetworkConnection {
            kind: "network".into(),
            address: address.to_string(),
        },
        source,
        printer_type: "FDM".into(),
    }
}

fn discover_moonraker(
    transport: &dyn ProbeTransport,
    address: Ipv4Addr,
    deadline: Instant,
    request_limit: Duration,
    cancelled: &AtomicBool,
) -> Option<Suggestion> {
    for port in [MOONRAKER_PORT, 80] {
        let info = match safe_get(
            transport,
            address,
            Scheme::Http,
            port,
            "/server/info",
            deadline,
            request_limit,
            Some(cancelled),
        ) {
            Ok(response) => response,
            Err(_) => continue,
        };
        let Some(info_json) = success_json(&info) else {
            continue;
        };
        if !is_moonraker_info(&info_json) {
            continue;
        }
        let printer_info = safe_get(
            transport,
            address,
            Scheme::Http,
            port,
            "/printer/info",
            deadline,
            request_limit,
            Some(cancelled),
        )
        .ok()
        .and_then(|response| success_json(&response));
        let result = printer_info.as_ref().and_then(|value| value.get("result"));
        let model = bounded_string(result.and_then(|value| value.get("model")), 128)
            .or_else(|| bounded_string(result.and_then(|value| value.get("software_version")), 64));
        let name = bounded_string(result.and_then(|value| value.get("hostname")), 80)
            .unwrap_or_else(|| format!("Klipper printer at {address}"));
        return Some(make_suggestion(
            address,
            SuggestionSource::Moonraker,
            name,
            model,
        ));
    }
    None
}

fn discover_prusalink(
    transport: &dyn ProbeTransport,
    address: Ipv4Addr,
    deadline: Instant,
    request_limit: Duration,
    cancelled: &AtomicBool,
) -> Option<Suggestion> {
    for (scheme, port) in [(Scheme::Https, 443), (Scheme::Http, 80)] {
        let response = match safe_get(
            transport,
            address,
            scheme,
            port,
            "/api/version",
            deadline,
            request_limit,
            Some(cancelled),
        ) {
            Ok(response) => response,
            Err(_) => continue,
        };
        let Some(value) = success_json(&response) else {
            continue;
        };
        if !is_prusalink_version(&value, &response.prusa_header_hints) {
            continue;
        }
        let model = bounded_string(value.get("printer"), 128)
            .or_else(|| bounded_string(value.get("model"), 128));
        let name = model
            .clone()
            .unwrap_or_else(|| format!("PrusaLink printer at {address}"));
        return Some(make_suggestion(
            address,
            SuggestionSource::Prusalink,
            name,
            model,
        ));
    }
    None
}

fn discover_host(
    transport: &dyn ProbeTransport,
    address: Ipv4Addr,
    bambu_candidates: &BTreeSet<Ipv4Addr>,
    deadline: Instant,
    request_limit: Duration,
    cancelled: &AtomicBool,
) -> Option<Suggestion> {
    if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
        return None;
    }
    if bambu_candidates.contains(&address)
        && probe_bambu_certificate(transport, address, deadline, request_limit).is_ok()
    {
        return Some(make_suggestion(
            address,
            SuggestionSource::BambuMqttTls,
            format!("Bambu Lab printer at {address}"),
            Some("Bambu Lab printer".into()),
        ));
    }
    if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
        return None;
    }
    if let Some(suggestion) =
        discover_moonraker(transport, address, deadline, request_limit, cancelled)
    {
        return Some(suggestion);
    }
    if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
        return None;
    }
    discover_prusalink(transport, address, deadline, request_limit, cancelled)
}

fn discover_with_transport(
    scope: &ScanScope,
    transport: Arc<dyn ProbeTransport>,
    cancelled: Arc<AtomicBool>,
    limits: ScanLimits,
) -> Vec<Suggestion> {
    discover_with_transport_options(scope, transport, cancelled, limits, true)
}
fn discover_with_transport_options(
    scope: &ScanScope,
    transport: Arc<dyn ProbeTransport>,
    cancelled: Arc<AtomicBool>,
    limits: ScanLimits,
    include_bambu: bool,
) -> Vec<Suggestion> {
    let limits = limits.bounded();
    let deadline = Instant::now() + limits.total;
    let addresses = scope.addresses().to_vec();
    let bambu_candidates = if include_bambu {
        self::bambu_discovery::validated_candidates(
            scope,
            transport.bambu_ssdp_responses(scope, deadline, limits.request, &cancelled),
        )
        .into_iter()
        .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    let next = AtomicUsize::new(0);
    let suggestions = Mutex::new(Vec::new());
    let workers = limits.workers.min(addresses.len()).max(1);
    thread::scope(|scope| {
        for _ in 0..workers {
            let transport = transport.clone();
            let cancelled = cancelled.clone();
            let suggestions = &suggestions;
            let next = &next;
            let addresses = &addresses;
            let bambu_candidates = &bambu_candidates;
            scope.spawn(move || loop {
                if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
                    break;
                }
                if suggestions
                    .lock()
                    .map(|items| items.len() >= limits.suggestions)
                    .unwrap_or(true)
                {
                    break;
                }
                let index = next.fetch_add(1, Ordering::AcqRel);
                let Some(address) = addresses.get(index).copied() else {
                    break;
                };
                let Some(suggestion) = discover_host(
                    transport.as_ref(),
                    address,
                    &bambu_candidates,
                    deadline,
                    limits.request,
                    &cancelled,
                ) else {
                    continue;
                };
                if let Ok(mut items) = suggestions.lock() {
                    if items.len() < limits.suggestions {
                        items.push(suggestion);
                    }
                }
            });
        }
    });
    suggestions.into_inner().unwrap_or_default()
}

#[derive(Debug, Clone, Copy)]
pub struct NetworkOutcome {
    pub result: PrinterConnectionTestResult,
    pub connection_status: PrinterConnectionStatus,
    pub job_status: PrinterJobStatus,
    pub error_code: Option<&'static str>,
    pub status_event_observed: bool,
}

impl NetworkOutcome {
    fn new(
        result: PrinterConnectionTestResult,
        connection_status: PrinterConnectionStatus,
        job_status: PrinterJobStatus,
        error_code: Option<&'static str>,
    ) -> Self {
        Self {
            result,
            connection_status,
            job_status,
            error_code,
            status_event_observed: false,
        }
    }

    fn failed(error_code: &'static str) -> Self {
        Self::new(
            PrinterConnectionTestResult::Failed,
            PrinterConnectionStatus::Error,
            PrinterJobStatus::Unknown,
            Some(error_code),
        )
    }
}

fn auth_required() -> NetworkOutcome {
    NetworkOutcome::new(
        PrinterConnectionTestResult::AuthRequired,
        PrinterConnectionStatus::AuthRequired,
        PrinterJobStatus::Unknown,
        Some("credentials_unavailable"),
    )
}

fn connection_succeeded(job_status: PrinterJobStatus) -> NetworkOutcome {
    NetworkOutcome::new(
        PrinterConnectionTestResult::Connected,
        PrinterConnectionStatus::Online,
        job_status,
        None,
    )
}

fn connection_succeeded_with_status_event(
    job_status: PrinterJobStatus,
    status_event_observed: bool,
) -> NetworkOutcome {
    let mut outcome = connection_succeeded(job_status);
    outcome.status_event_observed = status_event_observed;
    outcome
}

fn is_bambu_make(make: Option<&str>) -> bool {
    make.is_some_and(|make| {
        let make = make.to_ascii_lowercase();
        make.contains("bambu") || make.contains("bbl")
    })
}

fn is_prusa_make(make: Option<&str>) -> bool {
    make.is_some_and(|make| make.to_ascii_lowercase().contains("prusa"))
}

fn map_moonraker_job_status(value: &Value) -> PrinterJobStatus {
    match value
        .pointer("/result/status/print_stats/state")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "printing" => PrinterJobStatus::Printing,
        "paused" => PrinterJobStatus::Paused,
        "complete" | "completed" => PrinterJobStatus::Complete,
        "standby" | "ready" | "idle" => PrinterJobStatus::Idle,
        _ => PrinterJobStatus::Unknown,
    }
}

fn check_moonraker(
    transport: &dyn ProbeTransport,
    address: Ipv4Addr,
    deadline: Instant,
    request_limit: Duration,
) -> Option<NetworkOutcome> {
    for port in [MOONRAKER_PORT, 80] {
        let info = match safe_get(
            transport,
            address,
            Scheme::Http,
            port,
            "/server/info",
            deadline,
            request_limit,
            None,
        ) {
            Ok(response) => response,
            Err(_) => continue,
        };
        if auth_response(&info) {
            return Some(auth_required());
        }
        let Some(info_json) = success_json(&info) else {
            continue;
        };
        if !is_moonraker_info(&info_json) {
            continue;
        }
        let printer_info = match safe_get(
            transport,
            address,
            Scheme::Http,
            port,
            "/printer/info",
            deadline,
            request_limit,
            None,
        ) {
            Ok(response) => response,
            Err(_) => continue,
        };
        if auth_response(&printer_info) {
            return Some(auth_required());
        }
        if success_json(&printer_info)
            .as_ref()
            .and_then(|value| value.get("result"))
            .is_none()
        {
            continue;
        }
        let status = match safe_get(
            transport,
            address,
            Scheme::Http,
            port,
            "/printer/objects/query?print_stats&webhooks",
            deadline,
            request_limit,
            None,
        ) {
            Ok(response) => response,
            Err(_) => continue,
        };
        if auth_response(&status) {
            return Some(auth_required());
        }
        let Some(value) = success_json(&status) else {
            continue;
        };
        return Some(connection_succeeded(map_moonraker_job_status(&value)));
    }
    None
}

fn map_prusa_job_status(value: &Value) -> PrinterJobStatus {
    let text = value
        .pointer("/printer/state")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/state/text").and_then(Value::as_str))
        .unwrap_or_default()
        .to_ascii_lowercase();
    if text.contains("print") {
        PrinterJobStatus::Printing
    } else if text.contains("pause") {
        PrinterJobStatus::Paused
    } else if text.contains("complete") || text.contains("finished") {
        PrinterJobStatus::Complete
    } else if text.contains("idle") || text.contains("ready") {
        PrinterJobStatus::Idle
    } else {
        PrinterJobStatus::Unknown
    }
}

fn check_prusalink(
    transport: &dyn ProbeTransport,
    address: Ipv4Addr,
    deadline: Instant,
    request_limit: Duration,
) -> Option<NetworkOutcome> {
    for (scheme, port) in [(Scheme::Https, 443), (Scheme::Http, 80)] {
        let version = match safe_get(
            transport,
            address,
            scheme,
            port,
            "/api/version",
            deadline,
            request_limit,
            None,
        ) {
            Ok(response) => response,
            Err(_) => continue,
        };
        if auth_response(&version) {
            return Some(auth_required());
        }
        let Some(version_json) = success_json(&version) else {
            continue;
        };
        if !is_prusalink_version(&version_json, &version.prusa_header_hints) {
            continue;
        }
        let status = safe_get(
            transport,
            address,
            scheme,
            port,
            "/api/v1/status",
            deadline,
            request_limit,
            None,
        );
        return Some(match status {
            Ok(response) if response.status == 401 || response.status == 403 => auth_required(),
            Ok(response) => success_json(&response)
                .map(|value| connection_succeeded(map_prusa_job_status(&value)))
                .unwrap_or_else(|| {
                    NetworkOutcome::new(
                        PrinterConnectionTestResult::Unsupported,
                        PrinterConnectionStatus::Error,
                        PrinterJobStatus::Unknown,
                        Some("unsupported_response"),
                    )
                }),
            Err(ProbeError::Timeout | ProbeError::Unreachable) => NetworkOutcome::new(
                PrinterConnectionTestResult::Unreachable,
                PrinterConnectionStatus::Offline,
                PrinterJobStatus::Unknown,
                Some("unreachable"),
            ),
            Err(_) => NetworkOutcome::failed("connection_check_failed"),
        });
    }
    None
}

#[cfg(test)]
fn check_printer_with_transport(
    printer: &Printer,
    transport: &dyn ProbeTransport,
    deadline: Instant,
    request_limit: Duration,
) -> Result<NetworkOutcome, ProbeError> {
    check_printer_with_transport_and_sdcp(printer, transport, deadline, request_limit, None, false)
}

fn check_printer_with_transport_and_sdcp(
    printer: &Printer,
    transport: &dyn ProbeTransport,
    deadline: Instant,
    request_limit: Duration,
    mainboard_id: Option<&str>,
    discover_mainboard_id: bool,
) -> Result<NetworkOutcome, ProbeError> {
    if !printer.endpoint_approved {
        return Err(ProbeError::InvalidResponse);
    }
    let address = parse_printer_address(printer)?;
    if is_original_elegoo_centauri_carbon(printer) {
        let discovered_mainboard_id = if mainboard_id.is_none() && discover_mainboard_id {
            match transport
                .elegoo_sdcp_mainboard_id(address, bounded_timeout(deadline, request_limit)?)
            {
                Ok(mainboard_id) => Some(mainboard_id),
                Err(ProbeError::Timeout | ProbeError::Unreachable) => {
                    return Ok(NetworkOutcome::new(
                        PrinterConnectionTestResult::Unreachable,
                        PrinterConnectionStatus::Offline,
                        PrinterJobStatus::Unknown,
                        Some("unreachable"),
                    ));
                }
                Err(ProbeError::InvalidResponse) => {
                    return Ok(NetworkOutcome::failed("mainboard_id_invalid"));
                }
                Err(_) => return Ok(NetworkOutcome::failed("sdcp_discovery_failed")),
            }
        } else {
            None
        };
        let Some(mainboard_id) = mainboard_id.or(discovered_mainboard_id.as_deref()) else {
            return Ok(NetworkOutcome::failed("mainboard_id_required"));
        };
        let timeout = bounded_timeout(deadline, request_limit)?;
        return Ok(
            match transport.elegoo_sdcp_status_refresh(address, mainboard_id, timeout) {
                Ok(observation) => connection_succeeded_with_status_event(
                    observation.job_status,
                    observation.event_observed,
                ),
                Err(ProbeError::Timeout | ProbeError::Unreachable) => NetworkOutcome::new(
                    PrinterConnectionTestResult::Unreachable,
                    PrinterConnectionStatus::Offline,
                    PrinterJobStatus::Unknown,
                    Some("unreachable"),
                ),
                Err(ProbeError::InvalidResponse) => NetworkOutcome::failed("mainboard_id_invalid"),
                Err(ProbeError::AcknowledgementMissing) => {
                    NetworkOutcome::failed("sdcp_ack_unverified")
                }
                Err(_) => NetworkOutcome::failed("connection_check_failed"),
            },
        );
    }
    if discover_mainboard_id {
        return Err(ProbeError::InvalidResponse);
    }
    if is_bambu_make(printer.make.as_deref()) {
        return Ok(
            match probe_bambu_certificate(transport, address, deadline, request_limit) {
                Ok(_) => auth_required(),
                Err(ProbeError::Timeout | ProbeError::Unreachable) => NetworkOutcome::new(
                    PrinterConnectionTestResult::Unreachable,
                    PrinterConnectionStatus::Offline,
                    PrinterJobStatus::Unknown,
                    Some("unreachable"),
                ),
                Err(ProbeError::NotBambuCertificate) => NetworkOutcome::new(
                    PrinterConnectionTestResult::Unsupported,
                    PrinterConnectionStatus::Error,
                    PrinterJobStatus::Unknown,
                    Some("unsupported_protocol"),
                ),
                Err(_) => NetworkOutcome::failed("connection_check_failed"),
            },
        );
    }
    if is_prusa_make(printer.make.as_deref()) {
        if let Some(outcome) = check_prusalink(transport, address, deadline, request_limit) {
            return Ok(outcome);
        }
    }
    if let Some(outcome) = check_moonraker(transport, address, deadline, request_limit) {
        return Ok(outcome);
    }
    if !is_prusa_make(printer.make.as_deref()) {
        if let Some(outcome) = check_prusalink(transport, address, deadline, request_limit) {
            return Ok(outcome);
        }
    }
    match probe_bambu_certificate(transport, address, deadline, request_limit) {
        Ok(_) => Ok(auth_required()),
        Err(ProbeError::Timeout | ProbeError::Unreachable) => Ok(NetworkOutcome::new(
            PrinterConnectionTestResult::Unreachable,
            PrinterConnectionStatus::Offline,
            PrinterJobStatus::Unknown,
            Some("unreachable"),
        )),
        Err(ProbeError::NotBambuCertificate) => Ok(NetworkOutcome::new(
            PrinterConnectionTestResult::Unsupported,
            PrinterConnectionStatus::Error,
            PrinterJobStatus::Unknown,
            Some("unsupported_protocol"),
        )),
        Err(_) => Ok(NetworkOutcome::failed("connection_check_failed")),
    }
}

pub fn is_original_elegoo_centauri_carbon(printer: &Printer) -> bool {
    let is_elegoo = printer
        .make
        .as_deref()
        .is_some_and(|make| make.to_ascii_lowercase().contains("elegoo"));
    let model = printer
        .model
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    is_elegoo && model.contains("centauri carbon") && !model.contains("carbon 2")
}

pub fn same_printer_snapshot(expected: &Printer, current: Option<&Printer>) -> bool {
    current == Some(expected)
}

fn parse_bambu_certificate_identity(certificate: &[u8]) -> Option<BambuCertificateIdentity> {
    if certificate.len() > MAX_RESPONSE_BYTES {
        return None;
    }
    let (outer_tag, outer, after_outer) = der_tlv(certificate, 0)?;
    if outer_tag != 0x30 || after_outer != certificate.len() {
        return None;
    }
    let (tbs_tag, tbs, _) = der_tlv(outer, 0)?;
    if tbs_tag != 0x30 {
        return None;
    }
    let mut cursor = 0;
    let (tag, _, next) = der_tlv(tbs, cursor)?;
    if tag == 0xa0 {
        cursor = next;
    }
    for _ in 0..2 {
        let (_, _, next) = der_tlv(tbs, cursor)?;
        cursor = next;
    }
    let (issuer_tag, issuer, next) = der_tlv(tbs, cursor)?;
    if issuer_tag != 0x30 {
        return None;
    }
    cursor = next;
    let (_, _, next) = der_tlv(tbs, cursor)?;
    cursor = next;
    let (subject_tag, subject, _) = der_tlv(tbs, cursor)?;
    if subject_tag != 0x30 {
        return None;
    }
    let issuer_attributes = parse_name_attributes(issuer)?;
    let subject_attributes = parse_name_attributes(subject)?;
    let issuer_common_name = issuer_attributes
        .iter()
        .find(|(oid, _)| oid.as_slice() == [0x55, 0x04, 0x03])?
        .1
        .clone();
    let issuer_serialized = issuer_attributes
        .iter()
        .map(|(oid, value)| {
            let label = if oid.as_slice() == [0x55, 0x04, 0x03] {
                "CN"
            } else if oid.as_slice() == [0x55, 0x04, 0x0a] {
                "O"
            } else {
                "ATTR"
            };
            format!("{label}={value}")
        })
        .collect::<Vec<_>>()
        .join(",");
    let serial_number = subject_attributes
        .iter()
        .find(|(oid, _)| oid.as_slice() == [0x55, 0x04, 0x03])
        .map(|(_, value)| value)
        .filter(|value| {
            (12..=20).contains(&value.len())
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .cloned();
    Some(BambuCertificateIdentity {
        issuer_common_name,
        issuer_serialized,
        serial_number,
    })
}

fn der_tlv(input: &[u8], offset: usize) -> Option<(u8, &[u8], usize)> {
    let tag = *input.get(offset)?;
    let length_byte = *input.get(offset + 1)?;
    let (length, header_len) = if length_byte & 0x80 == 0 {
        (usize::from(length_byte), 2)
    } else {
        let count = usize::from(length_byte & 0x7f);
        if count == 0 || count > 4 {
            return None;
        }
        let mut length = 0_usize;
        for byte in input.get(offset + 2..offset + 2 + count)? {
            length = length.checked_mul(256)?.checked_add(usize::from(*byte))?;
        }
        if length < 128 {
            return None;
        }
        (length, 2 + count)
    };
    let start = offset.checked_add(header_len)?;
    let end = start.checked_add(length)?;
    Some((tag, input.get(start..end)?, end))
}

fn parse_name_attributes(name_content: &[u8]) -> Option<Vec<(Vec<u8>, String)>> {
    let mut attributes = Vec::new();
    let mut cursor = 0;
    while cursor < name_content.len() {
        let (set_tag, set, next) = der_tlv(name_content, cursor)?;
        if set_tag != 0x31 {
            return None;
        }
        cursor = next;
        let mut set_cursor = 0;
        while set_cursor < set.len() {
            let (sequence_tag, pair, pair_next) = der_tlv(set, set_cursor)?;
            if sequence_tag != 0x30 {
                return None;
            }
            set_cursor = pair_next;
            let (oid_tag, oid, oid_next) = der_tlv(pair, 0)?;
            if oid_tag != 0x06 {
                return None;
            }
            let (value_tag, value, value_next) = der_tlv(pair, oid_next)?;
            if value_next != pair.len() {
                return None;
            }
            let value = match value_tag {
                0x0c | 0x13 | 0x14 | 0x16 => String::from_utf8_lossy(value).into_owned(),
                0x1e if value.len() % 2 == 0 => value
                    .chunks_exact(2)
                    .filter_map(|pair| {
                        char::from_u32(u32::from(u16::from_be_bytes([pair[0], pair[1]])))
                    })
                    .collect(),
                _ => continue,
            };
            if !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control) {
                attributes.push((oid.to_vec(), value));
            }
        }
    }
    Some(attributes)
}

struct BambuCertificateVerifier {
    inner: Arc<rustls::client::WebPkiServerVerifier>,
    cross_signed_intermediates: Vec<rustls::pki_types::CertificateDer<'static>>,
    identity: Mutex<Option<BambuCertificateIdentity>>,
}

impl BambuCertificateVerifier {
    fn new(
        material: BambuCaMaterial,
        provider: Arc<rustls::crypto::CryptoProvider>,
    ) -> Result<Self, rustls::client::VerifierBuilderError> {
        let inner = rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(material.roots),
            provider,
        )
        .build()?;
        Ok(Self {
            inner,
            cross_signed_intermediates: material.cross_signed_intermediates,
            identity: Mutex::new(None),
        })
    }
}

impl fmt::Debug for BambuCertificateVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BambuCertificateVerifier")
    }
}

impl rustls::client::danger::ServerCertVerifier for BambuCertificateVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let mut chain = intermediates.to_vec();
        chain.extend(self.cross_signed_intermediates.iter().cloned());
        let verified =
            <rustls::client::WebPkiServerVerifier as rustls::client::danger::ServerCertVerifier>::verify_server_cert(
                self.inner.as_ref(),
                end_entity,
                &chain,
                server_name,
                ocsp_response,
                now,
            )?;
        let Some(identity) = parse_bambu_certificate_identity(end_entity.as_ref()) else {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            ));
        };
        let mut slot = self
            .identity
            .lock()
            .map_err(|_| rustls::Error::General("certificate verifier unavailable".into()))?;
        *slot = Some(identity);
        Ok(verified)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &rustls::pki_types::CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        <rustls::client::WebPkiServerVerifier as rustls::client::danger::ServerCertVerifier>::verify_tls12_signature(
            self.inner.as_ref(),
            message,
            certificate,
            signature,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &rustls::pki_types::CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        <rustls::client::WebPkiServerVerifier as rustls::client::danger::ServerCertVerifier>::verify_tls13_signature(
            self.inner.as_ref(),
            message,
            certificate,
            signature,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        <rustls::client::WebPkiServerVerifier as rustls::client::danger::ServerCertVerifier>::supported_verify_schemes(
            self.inner.as_ref(),
        )
    }
}

fn bambu_tls_certificate(
    address: Ipv4Addr,
    timeout: Duration,
    guard: &NetworkGuard,
) -> Result<BambuCertificateIdentity, ProbeError> {
    if !guard.permitted() {
        return Err(ProbeError::Failed);
    }
    if timeout.is_zero() {
        return Err(ProbeError::Timeout);
    }
    let started = Instant::now();
    let socket = SocketAddr::new(IpAddr::V4(address), BAMBU_MQTT_TLS_PORT);
    let stream = TcpStream::connect_timeout(&socket, timeout).map_err(classify_io_error)?;
    let remaining = timeout.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err(ProbeError::Timeout);
    }
    stream
        .set_read_timeout(Some(remaining))
        .map_err(classify_io_error)?;
    stream
        .set_write_timeout(Some(remaining))
        .map_err(classify_io_error)?;

    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = Arc::new(
        BambuCertificateVerifier::new(bambu_ca_material()?, provider.clone())
            .map_err(|_| ProbeError::Failed)?,
    );
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| ProbeError::Failed)?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    let server_name = rustls::pki_types::ServerName::IpAddress(address.into());
    let connection = rustls::ClientConnection::new(Arc::new(config), server_name)
        .map_err(|_| ProbeError::Failed)?;
    let mut tls = rustls::StreamOwned::new(connection, stream);
    while tls.conn.is_handshaking() {
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(ProbeError::Timeout);
        }
        tls.sock
            .set_read_timeout(Some(remaining))
            .map_err(classify_io_error)?;
        tls.sock
            .set_write_timeout(Some(remaining))
            .map_err(classify_io_error)?;
        tls.conn
            .complete_io(&mut tls.sock)
            .map_err(classify_io_error)?;
    }
    // No application protocol is sent: this closes immediately after the TLS
    // handshake and uses the certificate only as a candidate signal.
    let _ = tls.sock.shutdown(std::net::Shutdown::Both);
    let identity = {
        let mut slot = verifier.identity.lock().map_err(|_| ProbeError::Failed)?;
        slot.take()
    };
    identity.ok_or(ProbeError::NotBambuCertificate)
}

/// Read-only discovery only; creating this call does not save or approve candidates.
pub fn discover(
    scope: &ScanScope,
    cancelled: Arc<AtomicBool>,
    guard: NetworkGuard,
) -> Vec<Suggestion> {
    match NativeTransport::new(guard) {
        Ok(transport) => {
            discover_with_transport(scope, Arc::new(transport), cancelled, ScanLimits::default())
        }
        Err(_) => Vec::new(),
    }
}
/// Validates the same private IPv4 endpoint accepted by the native connection check.
pub fn printer_address(printer: &Printer) -> Option<Ipv4Addr> {
    parse_printer_address(printer).ok()
}

/// One explicit read-only check. Persistence remains the caller's conditional transaction.
pub fn check_connection(
    printer: &Printer,
    mainboard_id: Option<&str>,
    discover_mainboard_id: bool,
    guard: NetworkGuard,
) -> NetworkOutcome {
    match NativeTransport::new(guard) {
        Ok(transport) => check_printer_with_transport_and_sdcp(
            printer,
            &transport,
            Instant::now()
                + if discover_mainboard_id {
                    MAX_ELEGOO_DISCOVERY_TEST_DURATION
                } else {
                    MAX_REQUEST_DURATION
                },
            MAX_REQUEST_DURATION,
            mainboard_id,
            discover_mainboard_id,
        )
        .unwrap_or_else(|_| NetworkOutcome::failed("target_not_approved")),
        Err(_) => NetworkOutcome::failed("transport_unavailable"),
    }
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    #[test]
    fn disabled_host_guard_never_constructs_or_runs_a_native_transport() {
        let checks = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&checks);
        let guard = NetworkGuard::new(move || {
            observed.fetch_add(1, Ordering::AcqRel);
            false
        });
        assert!(matches!(
            NativeTransport::new(guard.clone()),
            Err(ProbeError::Failed)
        ));
        assert_eq!(checks.load(Ordering::Acquire), 1);
        let scope = parse_scan_scope("192.0.2.10");
        assert!(scope.is_err());
        // No socket can be reached: each operation stops at the denied constructor.
        let scope = parse_scan_scope("192.168.200.10").unwrap();
        assert!(discover(&scope, Arc::new(AtomicBool::new(false)), guard.clone()).is_empty());
        assert_eq!(checks.load(Ordering::Acquire), 2);
        let printer = tests::fixture_printer(None, "192.168.200.10", true);
        let outcome = check_connection(&printer, None, false, guard);
        assert_eq!(outcome.error_code, Some("transport_unavailable"));
        assert_eq!(checks.load(Ordering::Acquire), 3);
    }

    #[test]
    fn host_revocation_is_rechecked_at_every_transport_boundary() {
        let enabled = Arc::new(AtomicBool::new(true));
        let permission = Arc::clone(&enabled);
        let guard = NetworkGuard::new(move || permission.load(Ordering::Acquire));
        let transport = NativeTransport::new(guard.clone()).unwrap();
        enabled.store(false, Ordering::Release);
        // Addresses remain loopback-only even if a regression removes a guard.
        let loopback = Ipv4Addr::LOCALHOST;
        assert!(matches!(
            transport.get(
                ProbeRequest {
                    address: loopback,
                    scheme: Scheme::Http,
                    port: MOONRAKER_PORT,
                    path: "/server/info"
                },
                Duration::from_millis(1)
            ),
            Err(ProbeError::Failed)
        ));
        assert!(matches!(
            transport.bambu_certificate(loopback, Duration::from_millis(1)),
            Err(ProbeError::Failed)
        ));
        assert!(matches!(
            transport.elegoo_sdcp_status_refresh(
                loopback,
                "12345678901234567890123456789012",
                Duration::from_millis(1)
            ),
            Err(ProbeError::Failed)
        ));
        assert!(matches!(
            transport.elegoo_sdcp_mainboard_id(loopback, Duration::from_millis(1)),
            Err(ProbeError::Failed)
        ));
        let scope = parse_scan_scope("192.168.200.10").unwrap();
        assert!(transport
            .bambu_ssdp_responses(
                &scope,
                Instant::now(),
                Duration::ZERO,
                &AtomicBool::new(false)
            )
            .is_empty());
    }
}

/// Browser callers must explicitly choose multicast; native legacy calls retain their original choice.
pub fn discover_with_options(
    scope: &ScanScope,
    cancelled: Arc<AtomicBool>,
    guard: NetworkGuard,
    include_bambu: bool,
) -> Vec<Suggestion> {
    match NativeTransport::new(guard) {
        Ok(transport) => discover_with_transport_options(
            scope,
            Arc::new(transport),
            cancelled,
            ScanLimits::default(),
            include_bambu,
        ),
        Err(_) => Vec::new(),
    }
}
