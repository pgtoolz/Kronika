use super::{OsCpufreq, OsCpufreqPolicy};
use crate::{Section, StrId, Ts, contract::lint};

#[test]
fn contracts_are_policy_scoped_and_roundtrip_nulls() {
    assert_eq!(
        lint(&[OsCpufreqPolicy::CONTRACT, OsCpufreq::CONTRACT]),
        Ok(())
    );
    assert_eq!(OsCpufreqPolicy::CONTRACT.identity, ["policy_id"]);
    assert_eq!(OsCpufreq::CONTRACT.identity, ["policy_id"]);
    crate::assert_roundtrips(&[OsCpufreqPolicy {
        ts: Ts(10),
        policy_id: 3,
        related_cpus: Some(StrId(1)),
        scaling_driver: None,
        actual_source: Some(StrId(2)),
        cpuinfo_min_freq_hz: Some(800_000_000),
        cpuinfo_max_freq_hz: Some(3_600_000_000),
        scope: 0,
    }]);
    crate::assert_roundtrips(&[OsCpufreq {
        ts: Ts(20),
        policy_id: 3,
        actual_source: Some(StrId(2)),
        actual_frequency_hz: None,
        scaling_cur_freq_hz: Some(2_400_000_000),
        scaling_min_freq_hz: Some(800_000_000),
        scaling_max_freq_hz: Some(3_600_000_000),
        online_cpus: Some(4),
        scope: 0,
    }]);
}
