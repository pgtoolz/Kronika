Feature: What a PgBouncer log carries reaches the segment

  A client asking for a database the pooler does not have is refused, and
  PgBouncer writes that refusal twice: once as the reason the connection closed
  and once again as a pooler error, because `log_pooler_errors` is on by
  default. One refusal is one row.

  Scenario: A refused connection is one row, not two
    Given a PostgreSQL writing its log as stderr
    And a PgBouncer in front of it
    And the collector reaches PgBouncer by DSN
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
    When these clients connect through PgBouncer
      | database |
      | nope     |
    And it runs for 4 seconds
    Then some segment holds these sections
      | type_id | section          | min rows |
      | 2100001 | pgbouncer_events | 1        |
    And some segment records these log events exactly once
      | type_id | column | value                  |
      | 2100001 | text   | no such database: nope |

  Scenario: A glob finds the pooler's log without asking it anything
    Given a PostgreSQL writing its log as stderr
    And a PgBouncer in front of it
    And the collector is told the PgBouncer logs as /tmp/kronika-pgbouncer/*.log
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
    When these clients connect through PgBouncer
      | database |
      | nope     |
    And it runs for 4 seconds
    Then some segment records these log events
      | type_id | column      | value                                  |
      | 2100001 | text        | no such database: nope                 |
      | 2100001 | source_file | /tmp/kronika-pgbouncer/pgbouncer.log   |
