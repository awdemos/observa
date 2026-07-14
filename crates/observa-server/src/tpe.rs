//! Trusted Path Execution (TPE) detection and enforcement.
//!
//! TPE is a hardening concept: only allow the process (and its children) to
//! execute binaries from a small set of administrator-controlled directories.
//! This module:
//!
//! * detects whether the host kernel already enforces a TPE-like policy
//!   (grsecurity, fapolicyd, SELinux/AppArmor in enforcing mode, etc.);
//! * optionally enforces a Landlock sandbox that restricts execution to a
//!   configurable allow-list of system directories plus the Observa data/work
//!   directories.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Summary of system-level TPE-like protections.
#[derive(Debug, Clone, Serialize)]
pub struct TpeStatus {
    pub enabled: bool,
    pub mechanisms: Vec<String>,
    pub missing: Vec<String>,
    pub recommendation: String,
}

impl TpeStatus {
    pub fn not_available() -> Self {
        Self {
            enabled: false,
            mechanisms: Vec::new(),
            missing: vec!["Landlock".to_string()],
            recommendation: "Landlock is not available on this kernel. \
                TPE enforcement cannot be applied at runtime."
                .to_string(),
        }
    }
}

/// Inspect the running system for TPE-like protections.
///
/// This is best-effort: there is no single standard for TPE, so we look for
/// several common indicators.
pub fn detect_tpe() -> TpeStatus {
    let mut mechanisms = Vec::new();
    let mut missing = Vec::new();

    // 1. grsecurity classic TPE sysctl.
    if Path::new("/proc/sys/kernel/grsecurity/tpe").exists() {
        match std::fs::read_to_string("/proc/sys/kernel/grsecurity/tpe") {
            Ok(v) if v.trim() == "1" => mechanisms.push("grsecurity TPE".to_string()),
            _ => missing.push("grsecurity TPE is present but disabled".to_string()),
        }
    }

    // 2. Kernel command-line hints.
    if let Ok(cmdline) = std::fs::read_to_string("/proc/cmdline") {
        if cmdline.contains("tpe") || cmdline.contains("grsecurity") {
            mechanisms.push("TPE in kernel cmdline".to_string());
        }
    }

    // 3. LSM stack.
    if let Ok(lsm) = std::fs::read_to_string("/sys/kernel/security/lsm") {
        let lsm = lsm.trim();
        if !lsm.is_empty() {
            for name in lsm.split(',') {
                match name.trim() {
                    "apparmor" => mechanisms.push("AppArmor".to_string()),
                    "landlock" => mechanisms.push("Landlock LSM loaded".to_string()),
                    "yama" => mechanisms.push("Yama".to_string()),
                    _ => {}
                }
            }
        }
    }

    // 4. Protected symlinks/hardlinks are related hardening.
    if sysctl_bool("/proc/sys/fs/protected_symlinks") {
        mechanisms.push("protected symlinks".to_string());
    }
    if sysctl_bool("/proc/sys/fs/protected_hardlinks") {
        mechanisms.push("protected hardlinks".to_string());
    }

    // 5. File-access policy daemon.
    if fapolicyd_running() {
        mechanisms.push("fapolicyd".to_string());
    }

    // 6. Did we successfully apply our own Landlock sandbox?
    #[cfg(target_os = "linux")]
    if LANDLOCK_ENFORCED.load(std::sync::atomic::Ordering::Relaxed) {
        mechanisms.push("Observa Landlock sandbox".to_string());
    }

    let enabled = !mechanisms.is_empty();

    let recommendation = if enabled {
        "At least one TPE-like protection is active.".to_string()
    } else {
        "No TPE-like protection detected. Consider enabling AppArmor, fapolicyd, a grsecurity \
            kernel, or running Observa with its built-in Landlock sandbox."
            .to_string()
    };

    TpeStatus {
        enabled,
        mechanisms,
        missing,
        recommendation,
    }
}

fn sysctl_bool(path: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

fn fapolicyd_running() -> bool {
    // Best-effort: check the systemd unit state and the pid file.
    if Path::new("/run/systemd/units/fapolicyd.service").exists() {
        return true;
    }
    if Path::new("/run/fapolicyd.pid").exists() {
        return true;
    }
    false
}

#[cfg(target_os = "linux")]
static LANDLOCK_ENFORCED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Attempt to enforce a Landlock TPE sandbox.
///
/// `extra_trusted_dirs` are added to the default set of read+execute paths.
/// `data_dirs` are allowed read+write+execute.  Any failure is logged and
/// ignored so the app can still start on kernels that do not support Landlock.
#[cfg(target_os = "linux")]
pub fn enforce_tpe(
    extra_trusted_dirs: &[PathBuf],
    data_dirs: &[PathBuf],
) -> Result<(), observa_shared::ObservaError> {
    use landlock::{
        path_beneath_rules, Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd,
        Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, ABI,
    };

    let abi = ABI::V1;
    let ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| observa_shared::ObservaError::Config(format!("landlock ruleset: {e}")))?;

    let ro_access = AccessFs::from_read(abi);
    let rw_access = AccessFs::from_all(abi);

    // System directories from which binaries and libraries are loaded.
    let mut trusted: Vec<PathBuf> = vec![
        "/usr".into(),
        "/bin".into(),
        "/sbin".into(),
        "/lib".into(),
        "/lib64".into(),
        "/usr/bin".into(),
        "/usr/sbin".into(),
        "/usr/lib".into(),
        "/usr/lib64".into(),
        "/usr/local/bin".into(),
        "/usr/local/sbin".into(),
        "/usr/local/lib".into(),
        "/etc".into(),
        "/dev".into(),
        "/proc".into(),
        "/sys".into(),
        "/run".into(),
        "/var/log".into(),
    ];
    trusted.extend(extra_trusted_dirs.iter().cloned());

    // Add the directory containing the observa binary itself.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            trusted.push(parent.to_path_buf());
        }
    }

    // Add the current working directory so templates/assets/config files can be read.
    if let Ok(cwd) = std::env::current_dir() {
        trusted.push(cwd);
    }

    // Data directories need read+write (and execute for sqlite shm/wal files).
    let mut data: Vec<PathBuf> = vec!["/tmp".into(), "/var/tmp".into()];
    data.extend(data_dirs.iter().cloned());

    let ro_rules: Vec<Result<PathBeneath<PathFd>, landlock::RulesetError>> =
        path_beneath_rules(trusted.iter().map(|p| p.as_os_str()), ro_access).collect();

    let rw_rules: Vec<Result<PathBeneath<PathFd>, landlock::RulesetError>> =
        path_beneath_rules(data.iter().map(|p| p.as_os_str()), rw_access).collect();

    let status = ruleset
        .create()
        .map_err(|e| observa_shared::ObservaError::Config(format!("landlock create: {e}")))?
        .add_rules(ro_rules)
        .map_err(|e| observa_shared::ObservaError::Config(format!("landlock ro rules: {e}")))?
        .add_rules(rw_rules)
        .map_err(|e| observa_shared::ObservaError::Config(format!("landlock rw rules: {e}")))?
        .set_compatibility(CompatLevel::BestEffort)
        .restrict_self()
        .map_err(|e| observa_shared::ObservaError::Config(format!("landlock restrict: {e}")))?;

    if status.ruleset == RulesetStatus::FullyEnforced || status.ruleset == RulesetStatus::PartiallyEnforced
    {
        LANDLOCK_ENFORCED.store(true, std::sync::atomic::Ordering::Relaxed);
        tracing::info!("Landlock TPE sandbox enforced");
        Ok(())
    } else {
        Err(observa_shared::ObservaError::Config(
            "Landlock ruleset was not enforced".to_string(),
        ))
    }
}

/// No-op on non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn enforce_tpe(
    _extra_trusted_dirs: &[PathBuf],
    _data_dirs: &[PathBuf],
) -> Result<(), observa_shared::ObservaError> {
    Ok(())
}
