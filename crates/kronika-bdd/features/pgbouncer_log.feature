Feature: What a PgBouncer log carries reaches the segment

  A client asking for a database the pooler does not have is refused, and
  PgBouncer writes that refusal twice: once as the reason the connection closed
  and once again as a pooler error, because `log_pooler_errors` is on by
  default. Both messages retain their full text and connection context.

  Scenario: A refused connection preserves both logged messages
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
      | 2100002 | pgbouncer_events | 1        |
    And some segment records these log event prefixes exactly once
      | type_id | column | value                                         |
      | 2100002 | text   | closing because: no such database: nope (age= |
    And some segment records these log events exactly once
      | type_id | column | value                                |
      | 2100002 | text   | pooler error: no such database: nope |
    And some segment records these log events
      | type_id | column      | value                                |
      | 2100002 | source_file | /tmp/kronika-pgbouncer/pgbouncer.log |
      | 2100002 | database    | (nodb)                               |
      | 2100002 | username    | postgres                             |
      | 2100002 | host        | 127.0.0.1                            |
      | 2100002 | side        | C                                    |

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
    Then some segment records these log event prefixes exactly once
      | type_id | column | value                                         |
      | 2100002 | text   | closing because: no such database: nope (age= |
    And some segment records these log events exactly once
      | type_id | column | value                                |
      | 2100002 | text   | pooler error: no such database: nope |
    And some segment records these log events
      | type_id | column      | value                                |
      | 2100002 | source_file | /tmp/kronika-pgbouncer/pgbouncer.log |
      | 2100002 | database    | (nodb)                               |
      | 2100002 | username    | postgres                             |
      | 2100002 | host        | 127.0.0.1                            |
      | 2100002 | side        | C                                    |
