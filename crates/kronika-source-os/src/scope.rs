//! OS metric scope: what a value physically describes, so the reader never
//! mixes host-wide numbers with pod-local ones.

use crate::ProcFs;

/// Per-row scope tag. Stored as the `scope` `u8` column on every OS section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsScope {
    /// Node-wide metrics (physical or VM host).
    Host,
    /// Kubernetes pod (cgroup namespace).
    Pod,
    /// Kubernetes pod network namespace.
    PodNet,
    /// Container inside a pod or standalone runtime.
    Container,
    /// Scope could not be determined.
    Unknown,
}

impl OsScope {
    /// Stable on-disk encoding.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Host => 0,
            Self::Pod => 1,
            Self::PodNet => 2,
            Self::Container => 3,
            Self::Unknown => 4,
        }
    }
}

/// Whether `/proc/1/cgroup` content names a container runtime.
#[must_use]
pub(crate) fn detect_container_from_cgroup(cgroup: &str) -> bool {
    const MARKERS: [&str; 4] = ["kubepods", "docker", "containerd", "lxc"];
    cgroup
        .lines()
        .any(|line| MARKERS.iter().any(|m| line.contains(m)))
}

/// Multi-signal container detection.
///
/// When `KRONIKA_PROC_ROOT` is set (BDD fixtures or a node-agent pointed at
/// `/host/proc`), detection uses only the cgroup file read through that root.
/// The `/.dockerenv` and `KUBERNETES_SERVICE_HOST` signals describe the
/// collector's own packaging, not the root it is observing, so they are
/// skipped in that case.
///
/// When `KRONIKA_PROC_ROOT` is absent (default `/proc`), all three signals
/// are checked, matching Wave 1 production behavior.
#[must_use]
pub fn detect_container(fs: &ProcFs) -> bool {
    let proc_root_overridden = std::env::var_os("KRONIKA_PROC_ROOT").is_some();
    if !proc_root_overridden {
        if std::env::var_os("KUBERNETES_SERVICE_HOST").is_some() {
            return true;
        }
        if std::path::Path::new("/.dockerenv").exists() {
            return true;
        }
    }
    fs.read_raw("1/cgroup")
        .is_ok_and(|c| detect_container_from_cgroup(&c))
}

/// Maps the container flag to the appropriate network scope.
///
/// Pure function; tests can avoid env/filesystem non-determinism.
#[must_use]
pub const fn net_scope(in_container: bool) -> OsScope {
    if in_container {
        OsScope::PodNet
    } else {
        OsScope::Host
    }
}

#[cfg(test)]
mod tests {
    use super::{OsScope, detect_container_from_cgroup, net_scope};

    #[test]
    fn scope_encodes_as_stable_u8() {
        // The reader depends on these exact values; guard every variant.
        assert_eq!(OsScope::Host.as_u8(), 0);
        assert_eq!(OsScope::Pod.as_u8(), 1);
        assert_eq!(OsScope::PodNet.as_u8(), 2);
        assert_eq!(OsScope::Container.as_u8(), 3);
        assert_eq!(OsScope::Unknown.as_u8(), 4);
    }

    #[test]
    fn cgroup_markers_detect_a_container() {
        assert!(detect_container_from_cgroup("0::/kubepods/pod123/abc\n"));
        assert!(detect_container_from_cgroup("12:pids:/docker/deadbeef\n"));
        assert!(!detect_container_from_cgroup("0::/init.scope\n"));
    }

    #[test]
    fn net_scope_maps_container_flag() {
        assert_eq!(net_scope(true), OsScope::PodNet);
        assert_eq!(net_scope(false), OsScope::Host);
    }
}
