//! An explicit debug-only safety boundary for isolated Phase 8 visual QA.
//! The bundle identifier supplies Tauri's separate data/config paths. HOME is
//! never redirected; discovery that could read ordinary slicer data is disabled.

#[cfg(any(debug_assertions, test))]
use std::ffi::OsStr;

#[cfg(any(debug_assertions, test))]
pub(crate) const ENV_FLAG: &str = "GOGGLELAB_PHASE8_VISUAL_QA";
pub(crate) const BUNDLE_IDENTIFIER: &str = "com.sonicparke.gogglelab.phase8-qa";

#[cfg(any(debug_assertions, test))]
fn enabled_for(debug_build: bool, flag: Option<&OsStr>) -> bool {
    debug_build && flag == Some(OsStr::new("1"))
}

pub(crate) fn active() -> bool {
    // Release builds do not consult the environment at all.
    #[cfg(debug_assertions)]
    {
        enabled_for(true, std::env::var_os(ENV_FLAG).as_deref())
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

fn validate_identifier(enabled: bool, identifier: &str) -> std::io::Result<()> {
    if enabled && identifier != BUNDLE_IDENTIFIER {
        return Err(std::io::Error::other(
            "Phase 8 visual QA requires the isolated QA bundle identifier",
        ));
    }
    Ok(())
}

pub(crate) fn validate_startup(identifier: &str) -> std::io::Result<()> {
    validate_identifier(active(), identifier)
}

pub(crate) fn require_printer_network() -> Result<(), crate::extension_api::AppError> {
    if active() {
        return Err(crate::extension_api::AppError::Library {
            message: "Printer network access is disabled during isolated visual QA".into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_debug_opt_in_activates_isolation() {
        for flag in [
            None,
            Some(""),
            Some("0"),
            Some("true"),
            Some(" 1"),
            Some("1 "),
        ] {
            assert!(!enabled_for(true, flag.map(OsStr::new)));
        }
        assert!(enabled_for(true, Some(OsStr::new("1"))));
        assert!(!enabled_for(false, Some(OsStr::new("1"))));
    }

    #[test]
    fn qa_startup_requires_the_exact_isolated_identifier() {
        assert!(validate_identifier(true, BUNDLE_IDENTIFIER).is_ok());
        for identifier in [
            "com.sonicparke.stl-handoff",
            "",
            "com.sonicparke.gogglelab.phase8-qa.other",
        ] {
            assert!(validate_identifier(true, identifier).is_err());
            assert!(validate_identifier(false, identifier).is_ok());
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    fn isolated_process_disables_local_discovery_and_network() {
        // Use a child test process instead of mutating the environment while
        // other tests may be discovering synthetic slicer fixtures.
        if std::env::var_os("GOGGLELAB_QA_GUARD_TEST_CHILD").is_some() {
            assert!(active());
            assert!(crate::slicers::detect_slicers().is_empty());
            assert!(crate::printers::detect_printers().is_empty());
            for definition in crate::slicers::slicer_definitions() {
                assert!(crate::slicers::slicer_support_directory(definition).is_none());
            }
            assert!(require_printer_network().is_err());
            // The real socket entry points reject before route selection or
            // connection. Loopback inputs also make this safe on regressions.
            let address = std::net::Ipv4Addr::LOCALHOST;
            let timeout = std::time::Duration::from_millis(1);
            assert_eq!(
                crate::elegoo_discovery::discover_mainboard_id(address, timeout),
                Err(crate::elegoo_discovery::DiscoveryError::Failed)
            );
            assert_eq!(
                crate::elegoo_sdcp::send_status_refresh(address, "0123456789abcdef", timeout),
                Err(crate::elegoo_sdcp::SdcpRequestError::Io)
            );
            return;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "visual_qa::tests::isolated_process_disables_local_discovery_and_network",
            ])
            .env(ENV_FLAG, "1")
            .env("GOGGLELAB_QA_GUARD_TEST_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success());
    }
}
