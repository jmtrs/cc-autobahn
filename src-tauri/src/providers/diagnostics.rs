//! Capability/compatibility diagnostics: turns observed health plus the Codex
//! App Server snapshot into the per-provider capability matrix the Settings
//! diagnostics panel renders, and probes related on-disk Codex runtimes.
//!
//! Pure mapping (`build_provider_diagnostics` and friends) plus the bounded,
//! macOS-only bundle/version probing (`related_codex_runtimes`). The async
//! command wrapper that hydrates state and moves the probing off the UI thread
//! stays in the parent module.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
#[cfg(not(target_os = "macos"))]
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::codex;
use super::{
    CapabilityDiagnostic, HealthStatus, ProviderComponent, ProviderDiagnostics, ProviderHealth,
    ProviderId, RuntimeDiagnostic,
};

pub(super) fn build_provider_diagnostics(
    health: &HashMap<(ProviderId, ProviderComponent), ProviderHealth>,
    codex: codex::app_server::AccountSensorSnapshot,
    hook_active: bool,
    related_codex_runtimes: Vec<RuntimeDiagnostic>,
) -> Vec<ProviderDiagnostics> {
    let component_status =
        |provider, component| health.get(&(provider, component)).map(|value| value.status);
    let health_capability = |provider: ProviderId,
                             component: ProviderComponent,
                             id: &str,
                             source: &str,
                             quality: &str,
                             remediation: &str| {
        let status = component_status(provider, component);
        CapabilityDiagnostic {
            id: id.into(),
            status: match status {
                Some(HealthStatus::Connected) => "available",
                Some(HealthStatus::Degraded) => "degraded",
                Some(HealthStatus::Unavailable) => "unavailable",
                None => "unverified",
            }
            .into(),
            source: source.into(),
            quality: quality.into(),
            fallback: None,
            reason: health
                .get(&(provider, component))
                .and_then(|value| value.detail.clone()),
            remediation: (status != Some(HealthStatus::Connected)).then(|| remediation.to_string()),
        }
    };

    let claude_capabilities = vec![
        health_capability(
            ProviderId::Claude,
            ProviderComponent::Engine,
            "usage-history",
            "ccusage claude",
            "estimated",
            "Install or reconnect ccusage from Settings.",
        ),
        health_capability(
            ProviderId::Claude,
            ProviderComponent::Sensor,
            "limits",
            "Claude statusLine",
            "official",
            "Connect the Claude statusLine sensor.",
        ),
        health_capability(
            ProviderId::Claude,
            ProviderComponent::Transcript,
            "live-activity",
            "Claude transcript",
            "local",
            "Start a Claude Code session with transcript access.",
        ),
        health_capability(
            ProviderId::Claude,
            ProviderComponent::Permissions,
            "permissions",
            "Claude PermissionRequest hook",
            "native",
            "Connect the Claude permission hook from Settings.",
        ),
    ];
    let bool_capability = |id: &str,
                           available: Option<bool>,
                           source: &str,
                           quality: &str,
                           fallback: Option<&str>,
                           reason: Option<&str>,
                           remediation: &str| CapabilityDiagnostic {
        id: id.into(),
        status: match available {
            Some(true) => "available",
            Some(false) => "unavailable",
            None => "unverified",
        }
        .into(),
        source: source.into(),
        quality: quality.into(),
        fallback: fallback.map(str::to_string),
        reason: reason.map(str::to_string),
        remediation: (available != Some(true)).then(|| remediation.to_string()),
    };
    let transcript = component_status(ProviderId::Codex, ProviderComponent::Transcript);
    let history = component_status(ProviderId::Codex, ProviderComponent::History);
    let local_fallback_available = matches!(
        (transcript, history),
        (Some(HealthStatus::Connected), _) | (_, Some(HealthStatus::Connected))
    );
    let mut codex_capabilities = vec![
        bool_capability(
            "limits",
            codex.runtime.rate_limits_available,
            "Codex App Server",
            "official",
            None,
            codex.runtime.rate_limits_reason.as_deref(),
            "Use a compatible ChatGPT-authenticated Codex runtime.",
        ),
        bool_capability(
            "account-usage",
            codex.runtime.account_usage_available,
            "Codex App Server",
            "official",
            (history == Some(HealthStatus::Connected)).then_some("ccusage codex"),
            codex.runtime.account_usage_reason.as_deref(),
            "Use ChatGPT authentication or rely on estimated local history.",
        ),
        bool_capability(
            "hook-inventory",
            codex.runtime.hooks_inventory_available,
            "hooks/list",
            "native",
            None,
            codex.runtime.hooks_inventory_reason.as_deref(),
            "Open /hooks in Codex and review the user hook.",
        ),
    ];
    codex_capabilities.push(connection_capability(
        &codex.runtime,
        local_fallback_available,
    ));
    codex_capabilities.extend(permission_diagnostics(&codex, hook_active));
    codex_capabilities.push(CapabilityDiagnostic {
        id: "live-activity".into(),
        status: match transcript {
            Some(HealthStatus::Connected) => "available",
            Some(HealthStatus::Degraded) => "degraded",
            Some(HealthStatus::Unavailable) => "unavailable",
            None => "unverified",
        }
        .into(),
        source: "Codex rollout".into(),
        quality: "local".into(),
        fallback: None,
        reason: health
            .get(&(ProviderId::Codex, ProviderComponent::Transcript))
            .and_then(|value| value.detail.clone()),
        remediation: (transcript != Some(HealthStatus::Connected))
            .then(|| "Start a Codex session with a recognized local rollout.".into()),
    });
    let mut history_capability = health_capability(
        ProviderId::Codex,
        ProviderComponent::History,
        "history",
        "ccusage codex",
        "estimated",
        "Install or reconnect ccusage from Settings.",
    );
    if history == Some(HealthStatus::Degraded) {
        history_capability.status = "unavailable".into();
    }
    codex_capabilities.push(history_capability);
    let claude_compatibility = compatibility_for(&claude_capabilities);
    let codex_compatibility = compatibility_for(&codex_capabilities);

    vec![
        ProviderDiagnostics {
            provider: ProviderId::Claude,
            surface: "Claude Code · external sessions".into(),
            runtime_executable: None,
            runtime_version: None,
            related_runtimes: Vec::new(),
            compatibility: claude_compatibility.into(),
            capabilities: claude_capabilities,
        },
        ProviderDiagnostics {
            provider: ProviderId::Codex,
            surface: "Codex CLI · selected App Server runtime; ChatGPT desktop is independent"
                .into(),
            runtime_executable: codex.runtime.executable_path,
            runtime_version: codex.runtime.version,
            related_runtimes: related_codex_runtimes,
            compatibility: codex_compatibility.into(),
            capabilities: codex_capabilities,
        },
    ]
}

fn compatibility_for(capabilities: &[CapabilityDiagnostic]) -> &'static str {
    let relevant: Vec<_> = capabilities
        .iter()
        .filter(|capability| capability.id != "native-approval-fallback")
        .collect();
    if !relevant.is_empty()
        && relevant
            .iter()
            .all(|capability| capability.status == "available")
    {
        "compatible"
    } else if relevant.iter().any(|capability| {
        matches!(capability.status.as_str(), "available" | "degraded")
            || (capability.fallback.is_some() && !capability.id.starts_with("permission-hook-"))
    }) {
        "partial"
    } else {
        "unsupported"
    }
}

fn connection_capability(
    runtime: &codex::app_server::CodexRuntimeDiagnostics,
    local_fallback_available: bool,
) -> CapabilityDiagnostic {
    let connected = runtime.connection_status == "connected";
    CapabilityDiagnostic {
        id: "app-server-connection".into(),
        status: if connected {
            "available"
        } else if runtime.connection_status == "connecting" {
            "unverified"
        } else {
            "unavailable"
        }
        .into(),
        source: runtime
            .executable_path
            .clone()
            .unwrap_or_else(|| "selected Codex executable".into()),
        quality: "official".into(),
        fallback: local_fallback_available.then(|| "local rollout and ccusage".into()),
        reason: (!connected).then(|| runtime.connection_status.clone()),
        remediation: (!connected)
            .then(|| "Select a working Codex executable and verify authentication.".into()),
    }
}

fn permission_diagnostics(
    codex: &codex::app_server::AccountSensorSnapshot,
    hook_active: bool,
) -> Vec<CapabilityDiagnostic> {
    let probe = codex.permission_hook.as_ref();
    let inventoried = codex.runtime.hooks_inventory_available == Some(true);
    let state = |id: &str, available: bool, reason: String| CapabilityDiagnostic {
        id: id.into(),
        status: if available {
            "available"
        } else if inventoried {
            "unavailable"
        } else {
            "unverified"
        }
        .into(),
        source: probe
            .map(|probe| probe.source_path.clone())
            .unwrap_or_else(|| "hooks/list".into()),
        quality: "native".into(),
        fallback: Some("Codex native approval UI".into()),
        reason: (!available).then_some(reason),
        remediation: (!available).then(|| "Review the hook in Codex /hooks.".into()),
    };
    vec![
        state(
            "permission-hook-installed",
            probe.is_some(),
            "cc-autobahn hook not found in the inventoried configuration".into(),
        ),
        state(
            "permission-hook-enabled",
            probe.is_some_and(|probe| probe.enabled),
            "hook is disabled or not inventoried".into(),
        ),
        state(
            "permission-hook-trusted",
            probe.is_some_and(|probe| probe.trust_status == "trusted"),
            probe.map_or_else(
                || "hook trust is unverified".into(),
                |probe| format!("hook trust is {}", probe.trust_status),
            ),
        ),
        state(
            "permission-hook-active",
            hook_active,
            "no successful exchange for the current trusted hook hash".into(),
        ),
        CapabilityDiagnostic {
            id: "native-approval-fallback".into(),
            status: "available".into(),
            source: "Codex native approval UI".into(),
            quality: "native".into(),
            fallback: None,
            reason: None,
            remediation: None,
        },
    ]
}

#[cfg(target_os = "macos")]
pub(super) fn related_codex_runtimes() -> Vec<RuntimeDiagnostic> {
    [
        ("ChatGPT desktop", "/Applications/ChatGPT.app"),
        ("Codex desktop (compatibility)", "/Applications/Codex.app"),
    ]
    .into_iter()
    .filter_map(|(surface, bundle)| {
        let bundle = Path::new(bundle);
        bundle.is_dir().then(|| {
            let executable = bundle.join("Contents/Resources/codex");
            RuntimeDiagnostic {
                surface: surface.into(),
                product_version: bundle_product_version(bundle),
                runtime_executable: executable
                    .is_file()
                    .then(|| executable.to_string_lossy().into_owned()),
                runtime_version: executable
                    .is_file()
                    .then(|| command_version(&executable))
                    .flatten(),
            }
        })
    })
    .collect()
}

/// Linux: probe `$PATH` plus the common install locations for a `codex`
/// executable and report each unique one with its `--version`. No `.app`
/// bundles or `plutil` on Linux — the CLI is the only surface (D54).
#[cfg(not(target_os = "macos"))]
pub(super) fn related_codex_runtimes() -> Vec<RuntimeDiagnostic> {
    probe_codex_in(&codex_search_dirs())
}

/// Builds the search directory list from `$PATH` plus common Linux install
/// locations. Separate from the probe so the probe stays pure and testable.
#[cfg(not(target_os = "macos"))]
fn codex_search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = crate::env_lock::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(home) = crate::env_lock::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".cargo/bin"));
        dirs.push(home.join(".local/share/flatpak/exports/bin"));
    }
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/usr/bin"));
    dirs.push(PathBuf::from("/snap/bin"));
    dirs
}

/// One diagnostic per unique `codex` executable across `search_dirs`, deduped
/// by canonical path so symlinks/shims don't double-count the same binary.
/// Not pure — it shells out to `codex --version` (bounded by `command_version`)
/// and reads the filesystem — but its inputs are explicit (the dir list is a
/// parameter, not env I/O), so the discovery logic is unit-testable with a
/// temp dir holding a fake `codex`.
#[cfg(not(target_os = "macos"))]
fn probe_codex_in(search_dirs: &[PathBuf]) -> Vec<RuntimeDiagnostic> {
    use std::collections::HashSet;

    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out: Vec<RuntimeDiagnostic> = Vec::new();
    for dir in search_dirs {
        let candidate = dir.join("codex");
        if !candidate.is_file() {
            continue;
        }
        let canonical = std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if !seen.insert(canonical.clone()) {
            continue;
        }
        out.push(RuntimeDiagnostic {
            surface: "Codex CLI".into(),
            product_version: None,
            runtime_executable: Some(canonical.to_string_lossy().into_owned()),
            runtime_version: command_version(&canonical),
        });
    }
    out
}

#[cfg(target_os = "macos")]
fn bundle_product_version(bundle: &Path) -> Option<String> {
    let info = bundle.join("Contents/Info.plist");
    let mut command = Command::new("/usr/bin/plutil");
    command
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(info);
    bounded_stdout(&mut command, Duration::from_secs(2)).and_then(|bytes| output_text(&bytes))
}

fn command_version(executable: &Path) -> Option<String> {
    let mut command = Command::new(executable);
    command.arg("--version");
    bounded_stdout(&mut command, Duration::from_secs(2)).and_then(|bytes| output_text(&bytes))
}

fn bounded_stdout(command: &mut Command, timeout: Duration) -> Option<Vec<u8>> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut bytes = Vec::new();
    child.stdout.take()?.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn output_text(bytes: &[u8]) -> Option<String> {
    let value = String::from_utf8_lossy(bytes).trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_do_not_claim_unobserved_history_and_keep_runtime_identity() {
        let mut health = HashMap::new();
        health.insert(
            (ProviderId::Codex, ProviderComponent::Transcript),
            ProviderHealth {
                provider: ProviderId::Codex,
                component: ProviderComponent::Transcript,
                status: HealthStatus::Connected,
                observed_at_ms: 10,
                detail: None,
            },
        );
        let mut codex = codex::app_server::AccountSensorSnapshot::default();
        codex.runtime.executable_path = Some("/opt/codex".into());
        codex.runtime.version = Some("codex 1.2.3".into());
        codex.runtime.rate_limits_available = Some(false);

        let diagnostics = build_provider_diagnostics(&health, codex, false, Vec::new());
        let codex = diagnostics
            .iter()
            .find(|provider| provider.provider == ProviderId::Codex)
            .unwrap();
        assert_eq!(codex.runtime_executable.as_deref(), Some("/opt/codex"));
        assert_eq!(codex.compatibility, "partial");
        assert_eq!(
            codex
                .capabilities
                .iter()
                .find(|capability| capability.id == "history")
                .unwrap()
                .status,
            "unverified"
        );
    }

    #[test]
    fn compatibility_requires_every_relevant_capability_and_rejects_metadata_only() {
        let capability = |status: &str, fallback: Option<&str>| CapabilityDiagnostic {
            id: "test".into(),
            status: status.into(),
            source: "fixture".into(),
            quality: "official".into(),
            fallback: fallback.map(str::to_string),
            reason: None,
            remediation: None,
        };
        assert_eq!(
            compatibility_for(&[capability("available", None)]),
            "compatible"
        );
        assert_eq!(
            compatibility_for(&[
                capability("available", None),
                capability("unavailable", Some("local")),
            ]),
            "partial"
        );
        assert_eq!(
            compatibility_for(&[capability("unverified", None)]),
            "unsupported"
        );
        let mut permission_fallback = capability("unverified", Some("native UI"));
        permission_fallback.id = "permission-hook-active".into();
        assert_eq!(compatibility_for(&[permission_fallback]), "unsupported");
    }

    #[test]
    fn empty_codex_snapshot_is_unsupported_until_a_source_is_observed() {
        let diagnostics = build_provider_diagnostics(
            &HashMap::new(),
            codex::app_server::AccountSensorSnapshot::default(),
            false,
            Vec::new(),
        );
        let codex = diagnostics
            .iter()
            .find(|provider| provider.provider == ProviderId::Codex)
            .unwrap();
        assert_eq!(codex.compatibility, "unsupported");
    }

    #[test]
    fn failed_history_probe_does_not_count_as_an_observed_fallback() {
        let health = HashMap::from([(
            (ProviderId::Codex, ProviderComponent::History),
            ProviderHealth {
                provider: ProviderId::Codex,
                component: ProviderComponent::History,
                status: HealthStatus::Degraded,
                observed_at_ms: 10,
                detail: Some("ccusage failed".into()),
            },
        )]);
        let diagnostics = build_provider_diagnostics(
            &health,
            codex::app_server::AccountSensorSnapshot::default(),
            false,
            Vec::new(),
        );
        let codex = diagnostics
            .iter()
            .find(|provider| provider.provider == ProviderId::Codex)
            .unwrap();
        assert!(codex
            .capabilities
            .iter()
            .find(|capability| capability.id == "account-usage")
            .unwrap()
            .fallback
            .is_none());
        assert_eq!(codex.compatibility, "unsupported");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bundled_runtime_probe_has_a_hard_deadline() {
        let mut command = Command::new("/bin/sleep");
        command.arg("1");
        let started = Instant::now();
        assert!(bounded_stdout(&mut command, Duration::from_millis(20)).is_none());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn linux_codex_probe_finds_and_versions_a_codex_binary() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("ccab-codex-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let codex = dir.join("codex");
        std::fs::write(&codex, "#!/bin/sh\necho 'codex 0.0.0-test'\n").unwrap();
        let mut perms = std::fs::metadata(&codex).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&codex, perms).unwrap();

        let found = probe_codex_in(&[dir.clone()]);
        assert_eq!(found.len(), 1, "exactly one codex diagnostic expected");
        assert_eq!(found[0].surface, "Codex CLI");
        assert_eq!(
            found[0].runtime_version.as_deref(),
            Some("codex 0.0.0-test"),
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
