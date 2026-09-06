Feature: What a PostgreSQL server reports reaches the segment

  A real server answers the queries and the rows are asserted against the
  segment on disk, decoded back through the format, with dictionary text
  resolved.

  The section ids below name the layouts of the server this image installs
  (PostgreSQL 15). A different server major writes different layouts, and this
  scenario is meant to fail loudly rather than pass against a shape nobody
  asserted.

  The shutdown pg_query_summary line measures logical_application_estimate
  payload. Its positive query_rate_per_s is query_count divided by the real
  interval, allowing only interval_ms truncation and six-decimal rounding.
  application_payload_from_postgres_mib and
  application_payload_to_postgres_mib are their corresponding byte fields
  divided by 1,048,576 with the same rounding. errors equals query_errors plus
  sink_errors plus connect_errors; timeouts equals query_timeouts plus
  connect_timeouts. fetch_elapsed_ms_max never exceeds fetch_elapsed_ms_total.

  Scenario: A live server's statistics become typed rows
    Given a PostgreSQL writing its log as stderr
    And the collector reaches PostgreSQL by DSN
    And a collector with these settings
      | variable                             | value |
      | KRONIKA_INTERVAL_S                   | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES            | 1     |
      | KRONIKA_PG_INTERVAL_S                | 0     |
      | KRONIKA_PG_RELATIONS_INTERVAL_S      | 0     |
      | KRONIKA_LOG_INTERVAL_S               | 3600  |
      | KRONIKA_OS_CORE_INTERVAL_S           | 3600  |
      | KRONIKA_OS_MOUNTTOPO_INTERVAL_S      | 3600  |
      | KRONIKA_OS_PROCESS_INTERVAL_S        | 3600  |
      | KRONIKA_OS_PROCESS_STATUS_INTERVAL_S | 3600  |
      | KRONIKA_OS_CGROUP_INTERVAL_S         | 3600  |
      | KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S | 3600  |
    When these statements run against PostgreSQL
      | statement                                                      |
      | create table bdd_orders (id bigint primary key, payload text)  |
      | insert into bdd_orders select i, repeat('x', 64) from generate_series(1, 500) i |
      | analyze bdd_orders                                             |
      | select count(*) from bdd_orders where id < 100                 |
    And it runs for 4 seconds
    Then some segment holds these sections
      | type_id | section                | min rows |
      | 1019001 | pg_settings            | 100      |
      | 1008001 | pg_stat_archiver       | 1        |
      | 1007001 | pg_stat_wal            | 1        |
      | 1020001 | pg_wal_storage         | 1        |
      | 1005003 | pg_stat_database       | 1        |
      | 1001004 | pg_stat_activity       | 1        |
      | 1013006 | pg_stat_user_tables    | 1        |
      | 1014003 | pg_stat_user_indexes   | 1        |
    And some segment records these rows
      | type_id | column        | value          |
      | 1019001 | name          | shared_buffers |
      | 1005003 | datname       | postgres       |
      | 1013006 | relname       | bdd_orders     |
      | 1013006 | schemaname    | public         |
      | 1014003 | indexrelname  | bdd_orders_pkey |
      | 1014003 | amname        | btree          |
    And the shutdown pg_query_summary for logical_application_estimate payload reports consistent traffic and costs
      | field                                       | requirement |
      | interval_ms                                 | positive    |
      | query_count                                 | positive    |
      | rows                                        | positive    |
      | application_payload_from_postgres_bytes     | positive    |
      | application_payload_to_postgres_bytes       | positive    |
      | batches                                     | positive    |
      | encoded_bytes                               | positive    |
      | wal_bytes_appended                          | positive    |
      | peak_rss_kib                                | positive    |
      | query_errors                                | nonnegative |
      | sink_errors                                 | nonnegative |
      | connect_errors                              | nonnegative |
      | query_timeouts                              | nonnegative |
      | connect_timeouts                            | nonnegative |
      | errors                                      | nonnegative |
      | timeouts                                    | nonnegative |
      | slow_queries                                | nonnegative |
      | fetch_elapsed_ms_total                      | nonnegative |
      | fetch_elapsed_ms_max                        | nonnegative |
      | encode_elapsed_ms_total                     | nonnegative |
      | append_elapsed_ms_total                     | nonnegative |

  Scenario: Without a configured server no PostgreSQL section is written
    Given a collector with these settings
      | variable                        | value |
      | KRONIKA_INTERVAL_S              | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES       | 1     |
      | KRONIKA_PG_INTERVAL_S           | 0     |
      | KRONIKA_PG_RELATIONS_INTERVAL_S | 0     |
    When it runs for 3 seconds
    Then no segment holds these sections
      | type_id | section             |
      | 1019001 | pg_settings         |
      | 1005003 | pg_stat_database    |
      | 1013006 | pg_stat_user_tables |
