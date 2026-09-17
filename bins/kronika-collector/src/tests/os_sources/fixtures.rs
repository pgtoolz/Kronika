//! Row fixtures shared by collector integration and storage-cost tests.

use super::OsSources;
use kronika_registry::os_block_topology::OsBlockTopology;
use kronika_registry::os_cgroup_context::OsCgroupContextV2;
use kronika_registry::os_cpufreq::{OsCpufreq, OsCpufreqPolicy};
use kronika_registry::os_mountinfo::OsMountinfo;
use kronika_registry::os_user::OsUser;

impl OsSources {
    pub(crate) fn cgroup_context_only(row: &OsCgroupContextV2) -> Self {
        Self {
            cgroup_context: Some(*row),
            ..Self::default()
        }
    }

    pub(crate) fn cpufreq_only(policies: Vec<OsCpufreqPolicy>, samples: Vec<OsCpufreq>) -> Self {
        Self {
            cpufreq_policy: policies,
            cpufreq: samples,
            ..Self::default()
        }
    }

    pub(crate) fn storage_only(
        mounts: Vec<OsMountinfo>,
        block_topology: Vec<OsBlockTopology>,
    ) -> Self {
        Self {
            mountinfo: mounts,
            block_topology,
            ..Self::default()
        }
    }

    pub(crate) fn users_only(users: Vec<OsUser>) -> Self {
        Self {
            users,
            ..Self::default()
        }
    }
}
