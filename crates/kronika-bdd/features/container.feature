Feature: What the collector records about the container it runs in

  The suite runs inside Docker, so the collector under test really is in a
  container with a private cgroup namespace. Its visible root has --cpus=2; the 2 below has
  to match it, and a drift between the two fails this feature.

  Scenario: The segment says the collector was inside a container
    Given a collector with these settings
      | variable                  | value |
      | KRONIKA_INTERVAL_S        | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES | 1     |
    When it runs for 3 seconds
    Then every segment records these instance facts
      | column      | value |
      | environment | 1     |

  Scenario: Visible cgroups and the selected resource context reach the recordings
    Given a collector with these settings
      | variable                             | value |
      | KRONIKA_INTERVAL_S                   | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES            | 1     |
      | KRONIKA_OS_CGROUP_INTERVAL_S         | 0     |
      | KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S | 0     |
    When it runs for 3 seconds
    Then some segment holds these sections
      | type_id | section             | min rows |
      | 1200001 | os_cgroup_mapping   | 1        |
      | 1201003 | os_cgroup_cpu       | 1        |
      | 1202003 | os_cgroup_memory    | 1        |
      | 1203003 | os_cgroup_io        | 1        |
      | 1204001 | os_cgroup_pids      | 1        |
      | 1205002 | os_cgroup_context   | 1        |
      | 1206001 | os_cgroup_v2_group  | 1        |
      | 1207001 | os_cgroup_v2_cpu    | 1        |
      | 1208001 | os_cgroup_v2_memory | 1        |
      | 1209001 | os_cgroup_v2_pids   | 1        |
      | 1210001 | os_cgroup_v2_io     | 1        |
    And the log reports measured cgroup discovery costs

  Scenario: The private cgroup namespace root records its CPU limit
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
