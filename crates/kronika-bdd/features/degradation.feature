Feature: What happens to a metric the host will not give up

  The collector points at a fixture procfs that is missing files on purpose.
  A metric it cannot read is reported and then absent: no section, no zeros
  standing in for a reading nobody took.

  Scenario: A metric that cannot be read is reported with what the OS said
    Given a procfs fixture named procfs-without-meminfo
    And a collector with these settings
      | variable                    | value |
      | KRONIKA_INTERVAL_S          | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES   | 1     |
      | KRONIKA_OS_CORE_INTERVAL_S  | 0     |
    When it runs for 3 seconds
    Then the log reports these collections as degraded
      | collection | reason                                          |
      | os_meminfo | meminfo: No such file or directory (os error 2)  |
      | os_vmstat  | vmstat: No such file or directory (os error 2)  |
      | os_psi     | no pressure files available                     |
    And a segment exists under a YYYY/MM/DD directory

  Scenario: A metric that could not be read has no section, not a row of zeros
    Given a procfs fixture named procfs-without-meminfo
    And a collector with these settings
      | variable                    | value |
      | KRONIKA_INTERVAL_S          | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES   | 1     |
      | KRONIKA_OS_CORE_INTERVAL_S  | 0     |
    When it runs for 3 seconds
    Then no segment holds these sections
      | type_id | section       |
      | 1104001 | os_meminfo    |
      | 1106001 | os_vmstat     |
      | 1108001 | os_diskstats  |
      | 1114001 | os_interrupts |

  Scenario: The sources the fixture does provide are still collected
    Given a procfs fixture named procfs-without-meminfo
    And a collector with these settings
      | variable                    | value |
      | KRONIKA_INTERVAL_S          | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES   | 1     |
      | KRONIKA_OS_CORE_INTERVAL_S  | 0     |
    When it runs for 3 seconds
    Then every segment holds these sections
      | type_id | section           | min rows |
      | 1021002 | instance_metadata | 1        |
      | 1102001 | os_cpu            | 2        |
      | 1103001 | os_stat           | 1        |
      | 1105001 | os_loadavg        | 1        |
