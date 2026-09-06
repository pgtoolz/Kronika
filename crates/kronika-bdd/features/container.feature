Feature: What the collector records about the container it runs in

  The suite runs inside Docker, so the collector under test really is in a
  container under a cgroup limit. The image runs with --cpus=2; the 2 below has
  to match it, and a drift between the two fails this feature.

  Scenario: The segment says the collector was inside a container
    Given a collector with these settings
      | variable                    | value |
      | KRONIKA_INTERVAL_S          | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES   | 1     |
    When it runs for 3 seconds
    Then every segment records these instance facts
      | column      | value |
      | environment | 1     |

  Scenario: The cgroup the collector lives in reaches the segment
    Given a collector with these settings
      | variable                             | value |
      | KRONIKA_INTERVAL_S                   | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES            | 1     |
      | KRONIKA_OS_CGROUP_INTERVAL_S         | 0     |
      | KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S | 0     |
    When it runs for 3 seconds
    Then every segment holds these sections
      | type_id | section           | min rows |
      | 1200001 | os_cgroup_mapping | 1        |
      | 1201001 | os_cgroup_cpu     | 1        |
      | 1202001 | os_cgroup_memory  | 1        |
      | 1203002 | os_cgroup_io      | 1        |
      | 1204001 | os_cgroup_pids    | 1        |
      | 1205001 | os_cgroup_context | 1        |

  Scenario: The recorded CPU limit is the container's, not the host's core count
    Given a collector with these settings
      | variable                     | value |
      | KRONIKA_INTERVAL_S           | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES    | 1     |
      | KRONIKA_OS_CGROUP_INTERVAL_S | 0     |
    When it runs for 3 seconds
    Then some segment records a cgroup CPU limit of 2 cores

  Scenario: A full collection stays inside the memory limit
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
    When it runs for 5 seconds
    Then its peak RSS stays under 25 MiB
