use std::fs;

use tempfile::tempdir;

use super::{ActualFrequencySource, collect};
use crate::SysFs;

#[test]
fn policies_prefer_average_and_keep_scaling_frequency_separate() {
    let directory = tempdir().expect("create CPUFreq root");
    let policy = directory.path().join("devices/system/cpu/cpufreq/policy2");
    fs::create_dir_all(&policy).expect("create policy");
    fs::create_dir_all(directory.path().join("devices/system/cpu")).expect("create CPU root");
    fs::write(directory.path().join("devices/system/cpu/online"), "0-3\n")
        .expect("write online CPUs");
    for (name, value) in [
        ("related_cpus", "0-3\n"),
        ("affected_cpus", "0 2\n"),
        ("scaling_driver", "intel_pstate\n"),
        ("cpuinfo_avg_freq", "2450000\n"),
        ("cpuinfo_cur_freq", "2300000\n"),
        ("cpuinfo_min_freq", "800000\n"),
        ("cpuinfo_max_freq", "3600000\n"),
        ("scaling_cur_freq", "2200000\n"),
        ("scaling_min_freq", "1000000\n"),
        ("scaling_max_freq", "3200000\n"),
    ] {
        fs::write(policy.join(name), value).expect("write policy attribute");
    }

    let observed =
        collect(&SysFs::new(directory.path().to_path_buf()), true, true).expect("collect");
    assert_eq!(observed.policies.len(), 1);
    assert_eq!(observed.policies[0].policy_id, 2);
    assert_eq!(
        observed.policies[0].actual_source,
        ActualFrequencySource::CpuinfoAverage
    );
    assert_eq!(observed.policies[0].related_cpus.as_deref(), Some("0-3"));
    assert_eq!(observed.samples[0].actual_frequency_hz, Some(2_450_000_000));
    assert_eq!(observed.samples[0].scaling_cur_freq_hz, Some(2_200_000_000));
    assert_eq!(observed.samples[0].online_cpus, Some(2));
}

#[test]
fn a_malformed_average_falls_through_to_current() {
    let directory = tempdir().expect("create CPUFreq root");
    let policy = directory.path().join("devices/system/cpu/cpufreq/policy0");
    fs::create_dir_all(&policy).expect("create policy");
    fs::write(policy.join("cpuinfo_avg_freq"), "not-a-frequency\n")
        .expect("write malformed average");
    fs::write(policy.join("cpuinfo_cur_freq"), "2300000\n").expect("write current");

    let observed =
        collect(&SysFs::new(directory.path().to_path_buf()), true, true).expect("collect");
    assert_eq!(
        observed.policies[0].actual_source,
        ActualFrequencySource::CpuinfoCurrent
    );
    assert_eq!(observed.samples[0].actual_frequency_hz, Some(2_300_000_000));
}

#[test]
fn each_observation_uses_the_first_source_it_can_parse() {
    let directory = tempdir().expect("create CPUFreq root");
    let policy = directory.path().join("devices/system/cpu/cpufreq/policy0");
    fs::create_dir_all(&policy).expect("create policy");
    fs::write(policy.join("cpuinfo_avg_freq"), "2400000\n").expect("write average");
    fs::write(policy.join("cpuinfo_cur_freq"), "2300000\n").expect("write current");
    let sys = SysFs::new(directory.path().to_path_buf());
    let first = collect(&sys, true, true).expect("collect first sample");
    assert_eq!(first.samples[0].actual_frequency_hz, Some(2_400_000_000));

    fs::remove_file(policy.join("cpuinfo_avg_freq")).expect("remove chosen average");
    let second = collect(&sys, true, true).expect("collect second sample");
    assert_eq!(
        second.policies[0].actual_source,
        ActualFrequencySource::CpuinfoCurrent
    );
    assert_eq!(second.samples[0].actual_frequency_hz, Some(2_300_000_000));
}

#[test]
fn unavailable_policy_attributes_remain_absent() {
    let dir = tempdir().expect("sysfs root");
    fs::create_dir_all(dir.path().join("devices/system/cpu/cpufreq/policy3"))
        .expect("empty policy");
    let sys = SysFs::new(dir.path().to_path_buf());
    let observed = collect(&sys, true, true).expect("collect policy");
    let mut intern = |_: &str| -> Result<kronika_registry::StrId, std::convert::Infallible> {
        panic!("absent attributes must not reach the dictionary")
    };
    let policy = super::policy_row(&observed.policies[0], &mut intern, 0, 9).expect("policy row");
    let sample = super::sample_row(&observed.samples[0], &mut intern, 0, 9).expect("sample row");
    assert_eq!((policy.policy_id, sample.policy_id), (3, 3));
    assert_eq!(policy.related_cpus, None);
    assert_eq!(policy.scaling_driver, None);
    assert_eq!((policy.actual_source, sample.actual_source), (None, None));
    assert_eq!(sample.online_cpus, None);
    for value in [
        policy.cpuinfo_min_freq_hz,
        policy.cpuinfo_max_freq_hz,
        sample.actual_frequency_hz,
        sample.scaling_cur_freq_hz,
        sample.scaling_min_freq_hz,
        sample.scaling_max_freq_hz,
    ] {
        assert_eq!(value, None);
    }
}
