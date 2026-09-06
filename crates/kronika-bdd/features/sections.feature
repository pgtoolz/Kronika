Feature: Which sections reach the segment, and how many rows they may hold

  Every source runs on the same one-second interval here, so each tick collects
  everything and every segment carries the full set. The list below is written
  out by hand rather than derived from the registry: a section that stops being
  collected has to fail a test, not quietly leave a generated list.

  Scenario: Every section the collector reads from this host reaches the segment
    Given a collector with these settings
      | variable                             | value |
      | KRONIKA_INTERVAL_S                   | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES            | 1     |
      | KRONIKA_OS_CORE_INTERVAL_S           | 0     |
      | KRONIKA_OS_MOUNTTOPO_INTERVAL_S      | 0     |
      | KRONIKA_OS_PROCESS_INTERVAL_S        | 0     |
      | KRONIKA_OS_PROCESS_STATUS_INTERVAL_S | 0     |
      | KRONIKA_OS_CGROUP_INTERVAL_S         | 0     |
      | KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S | 0     |
    When it runs for 3 seconds
    Then every segment holds these sections
      | type_id | section            | min rows |
      | 1021002 | instance_metadata  | 1        |
      | 1100001 | os_process         | 1        |
      | 1101001 | os_process_status  | 1        |
      | 1102001 | os_cpu             | 2        |
      | 1103001 | os_stat            | 1        |
      | 1104001 | os_meminfo         | 1        |
      | 1105001 | os_loadavg         | 1        |
      | 1106001 | os_vmstat          | 1        |
      | 1107001 | os_psi             | 3        |
      | 1109001 | os_netdev          | 1        |
      | 1110001 | os_snmp            | 1        |
      | 1111001 | os_netstat         | 1        |
      | 1112002 | os_mountinfo       | 1        |
      | 1113001 | os_topology        | 1        |
      | 1115001 | os_softirq         | 1        |
      | 1116001 | os_kernel_limits   | 1        |
      | 1117001 | os_numa            | 1        |
      | 1118001 | os_snmp6           | 1        |
      | 1200001 | os_cgroup_mapping  | 1        |
      | 1201001 | os_cgroup_cpu      | 1        |
      | 1202001 | os_cgroup_memory   | 1        |
      | 1203002 | os_cgroup_io       | 1        |
      | 1204001 | os_cgroup_pids     | 1        |
      | 1205001 | os_cgroup_context  | 1        |
    And every segment holds these sections
      | type_id | section      | min rows |
      | 3001001 | dict.strings | 1        |
