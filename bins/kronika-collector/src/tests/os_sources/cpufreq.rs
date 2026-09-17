use super::{DueSet, Interner, OsSources, SourceKind, SysFs, collect_cpufreq};
use kronika_format::{DictLimits, StrId as DictStrId};
use kronika_registry::StrId;
use std::path::Path;

fn write_policy(root: &Path, id: i32, attributes: &[(&str, &str)]) {
    let policy = root.join(format!("devices/system/cpu/cpufreq/policy{id}"));
    std::fs::create_dir_all(&policy).expect("create policy");
    for (name, value) in attributes {
        std::fs::write(policy.join(name), value).expect("write policy attribute");
    }
}

fn resolved_bytes(interner: &Interner, id: Option<StrId>) -> &[u8] {
    let id = DictStrId::from_raw(id.expect("recorded string ID").0).expect("nonzero ID");
    interner
        .window()
        .resolve(id)
        .expect("string present in dictionary")
        .stored_bytes()
}

#[test]
fn references_and_samples_follow_independent_schedules() {
    let dir = tempfile::tempdir().expect("sysfs root");
    write_policy(
        dir.path(),
        4,
        &[
            ("related_cpus", "0 2"),
            ("scaling_driver", "intel_pstate"),
            ("cpuinfo_avg_freq", "2500000"),
            ("scaling_cur_freq", "2100000"),
        ],
    );
    let sys = SysFs::new(dir.path().to_path_buf());
    for (kinds, references, samples) in [
        (vec![], 0, 0),
        (vec![SourceKind::OsMountTopo], 1, 0),
        (vec![SourceKind::OsCore], 0, 1),
        (vec![SourceKind::OsMountTopo, SourceKind::OsCore], 1, 1),
    ] {
        let mut interner = Interner::new(DictLimits::default());
        let mut os = OsSources::default();

        collect_cpufreq(&sys, &mut interner, 4, 7, &DueSet::for_test(kinds), &mut os);

        assert_eq!(
            (os.cpufreq_policy.len(), os.cpufreq.len()),
            (references, samples)
        );
        for row in &os.cpufreq_policy {
            assert_eq!((row.policy_id, row.ts.0, row.scope), (4, 7, 4));
            assert_eq!(
                resolved_bytes(&interner, row.scaling_driver),
                b"intel_pstate"
            );
        }
        for row in &os.cpufreq {
            assert_eq!((row.policy_id, row.ts.0, row.scope), (4, 7, 4));
            assert_eq!(row.actual_frequency_hz, Some(2_500_000_000));
            assert_eq!(row.scaling_cur_freq_hz, Some(2_100_000_000));
            assert_eq!(
                resolved_bytes(&interner, row.actual_source),
                b"cpuinfo_avg_freq"
            );
        }
    }
}

#[test]
fn dictionary_failure_skips_only_rows_needing_new_strings() {
    for (attribute, value, actual_source, expected_samples) in [
        ("related_cpus", "0", "cpuinfo_avg_freq", &[0, 1, 2][..]),
        (
            "scaling_driver",
            "new_driver",
            "cpuinfo_avg_freq",
            &[0, 1, 2][..],
        ),
        (
            "cpuinfo_cur_freq",
            "2000000",
            "cpuinfo_cur_freq",
            &[1, 2][..],
        ),
    ] {
        let dir = tempfile::tempdir().expect("sysfs root");
        write_policy(
            dir.path(),
            0,
            &[(actual_source, "2000000"), (attribute, value)],
        );
        write_policy(dir.path(), 1, &[("cpuinfo_avg_freq", "2500000")]);
        write_policy(dir.path(), 2, &[]);
        let sys = SysFs::new(dir.path().to_path_buf());
        let limits = DictLimits::new(16, 16)
            .expect("valid string limits")
            .with_max_total_bytes(16)
            .expect("one source name fits");
        let mut interner = Interner::new(limits);
        let source = interner
            .intern(b"cpuinfo_avg_freq")
            .expect("fill dictionary");
        let mut os = OsSources::default();
        let due = DueSet::for_test(vec![SourceKind::OsMountTopo, SourceKind::OsCore]);

        collect_cpufreq(&sys, &mut interner, 0, 9, &due, &mut os);

        assert_eq!(
            os.cpufreq_policy
                .iter()
                .map(|row| row.policy_id)
                .collect::<Vec<_>>(),
            [1, 2],
            "failed to intern {attribute}"
        );
        assert_eq!(
            os.cpufreq
                .iter()
                .map(|row| row.policy_id)
                .collect::<Vec<_>>(),
            expected_samples,
            "samples survive reference-only {attribute} failures"
        );
        assert_eq!(
            os.cpufreq_policy[0].actual_source,
            Some(StrId(source.get()))
        );
        assert_eq!(os.cpufreq_policy[1].actual_source, None);
        for sample in &os.cpufreq {
            let expected_source = (sample.policy_id != 2).then_some(StrId(source.get()));
            assert_eq!(sample.actual_source, expected_source);
        }
    }
}
