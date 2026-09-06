//! Bounded Linux operating-system collectors and parsers.
//!
//! [`ProcFs`] and [`SysFs`] restrict relative paths to configured roots and
//! cap each text read at [`MAX_PROC_FILE_BYTES`]. Parsers cover CPU, memory,
//! disk, network, pressure, mount, process, filesystem, and cgroup v1/v2 data;
//! conversion helpers produce `kronika-registry` rows where the registry owns
//! the on-disk contract.
//!
//! Collection is intentionally partial under races and permission limits:
//! disappearing processes and unreadable optional files become missing fields
//! or per-source diagnostics in the collector. Directory cardinality, cgroup
//! depth, disk count, and process count are bounded by the caller. Fixture root
//! overrides exist for BDD; production defaults are `/proc` and `/sys`.
//!
//! The crate is Linux-specific and does not own scheduling, interning, segment
//! state, or HTTP serialization.

pub mod block_topology;
pub mod cgroup;
pub mod cpufreq;

mod fs;
pub use fs::{
    DirEntryName, FsSpace, MAX_PROC_FILE_BYTES, ProcFs, SysFs, parse_dev_pair, space_from_raw,
    statvfs,
};

mod instance;
pub use instance::{OsInstanceFacts, collect_os_instance_facts};

pub mod mount;
pub use mount::{
    MountEntry, MountStringIds, container_device_set, device_map, display_path, is_k8s_infra_mount,
    is_kernel_tree_mount, is_pseudo_filesystem, mount_row, parse_mountinfo,
};

pub mod numa;
pub use numa::{node_id_from_dir, parse_node_meminfo};

pub mod proc;
pub use proc::stat::{CpuRow, ParseError, parse_cpu};

pub mod passwd;
pub use passwd::PasswdSnapshot;

mod scope;
pub use scope::{OsScope, detect_container, net_scope};
