Feature: What a PostgreSQL log carries reaches the segment

  A real server writes the log and the collector reads whichever of the three
  shapes `log_destination` produced. Every row below is asserted against the
  segment on disk, decoded back through the format, with dictionary text
  resolved: not against the lines the collector logged about writing it.

  The three destinations carry the same records, so they assert the same rows.

  Scenario Outline: A live server's log becomes typed events
    Given a PostgreSQL writing its log as <destination>
    And the collector reaches PostgreSQL by DSN
    And a collector with these settings
      | variable                             | value |
      | KRONIKA_INTERVAL_S                   | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES            | 1     |
      | KRONIKA_LOG_INTERVAL_S               | 0     |
      | KRONIKA_INSTANCE_INTERVAL_S          | 0     |
      | KRONIKA_OS_CORE_INTERVAL_S           | 3600  |
      | KRONIKA_OS_MOUNTTOPO_INTERVAL_S      | 3600  |
      | KRONIKA_OS_PROCESS_INTERVAL_S        | 3600  |
      | KRONIKA_OS_PROCESS_STATUS_INTERVAL_S | 3600  |
      | KRONIKA_OS_CGROUP_INTERVAL_S         | 3600  |
      | KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S | 3600  |
    When these statements run against PostgreSQL
      | statement                                                                                            |
      | select * from missing_table                                                                          |
      | checkpoint                                                                                           |
      | select pg_sleep(0.2)                                                                                 |
      | set work_mem = '64kB'; select count(*) from (select i from generate_series(1, 200000) i order by i) t |
    And it runs for 4 seconds
    Then some segment holds these sections
      | type_id | section             | min rows |
      | 2001001 | pg_log_errors       | 1        |
      | 2002001 | pg_log_checkpoints  | 1        |
      | 2004001 | pg_log_slow_queries | 1        |
      | 2006001 | pg_log_lifecycle    | 1        |
      | 2007001 | pg_log_temp_files   | 1        |
    And some segment records these log events
      | type_id | column   | value                        |
      | 2001001 | pattern  | relation "..." does not exist |
      | 2001001 | severity | 0                            |
      | 2001001 | category | 9                            |
      | 2002001 | phase    | 1                            |
      | 2004001 | pattern  | select pg_sleep(...)         |
      | 2006001 | kind     | 2                            |

    Examples:
      | destination |
      | stderr      |
      | csvlog      |
      | jsonlog     |

  Scenario: A log named outright is read without asking the server
    Given a PostgreSQL writing its log as csvlog
    And the collector is told the PostgreSQL log path
    And a collector with these settings
      | variable                             | value |
      | KRONIKA_INTERVAL_S                   | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES            | 1     |
      | KRONIKA_LOG_INTERVAL_S               | 0     |
      | KRONIKA_INSTANCE_INTERVAL_S          | 0     |
      | KRONIKA_OS_CORE_INTERVAL_S           | 3600  |
      | KRONIKA_OS_MOUNTTOPO_INTERVAL_S      | 3600  |
      | KRONIKA_OS_PROCESS_INTERVAL_S        | 3600  |
      | KRONIKA_OS_PROCESS_STATUS_INTERVAL_S | 3600  |
      | KRONIKA_OS_CGROUP_INTERVAL_S         | 3600  |
      | KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S | 3600  |
    When these statements run against PostgreSQL
      | statement                   |
      | select * from missing_table |
    And it runs for 4 seconds
    Then some segment holds these sections
      | type_id | section       | min rows |
      | 2001001 | pg_log_errors | 1        |
    And some segment records these log events
      | type_id | column            | value |
      | 2001001 | system_identifier | null  |
