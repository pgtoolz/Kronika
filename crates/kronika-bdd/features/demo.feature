Feature: The demo runs bounded commerce, plan-change, and system workloads

  Scenario: A sequential-scan regression returns to the indexed baseline
    Given a demo workload with these settings
      | variable                                      | value |
      | KRONIKA_DEMO_DURATION_S                       | 8     |
      | KRONIKA_INTERVAL_S                            | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES                     | 1     |
      | KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED          | false |
      | KRONIKA_DEMO_WORKLOAD_SCHEMAS                 | 1     |
      | KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA       | 8     |
      | KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY         | 1     |
      | KRONIKA_DEMO_WORKLOAD_SESSIONS                | 1     |
      | KRONIKA_DEMO_WORKLOAD_TPS                     | 10    |
      | KRONIKA_DEMO_WORKLOAD_MAX_ORDERS              | 4     |
      | KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS             | 1     |
      | KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH        | 4     |
      | KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS            | 4000  |
      | KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S   | 120   |
      | KRONIKA_DEMO_WORKLOAD_EVENT_ROUND_INTERVAL_S  | 180   |
      | KRONIKA_DEMO_WORKLOAD_PLAN_ROWS               | 30000 |
      | KRONIKA_DEMO_WORKLOAD_PLAN_WORKERS            | 1     |
      | KRONIKA_DEMO_WORKLOAD_PLAN_BASELINE_S         | 1     |
      | KRONIKA_DEMO_WORKLOAD_PLAN_REGRESSION_S       | 2     |
      | KRONIKA_DEMO_WORKLOAD_PLAN_ROUND_INTERVAL_S   | 120   |
      | KRONIKA_DEMO_WORKLOAD_VACUUM_ROWS             | 100   |
      | KRONIKA_DEMO_WORKLOAD_VACUUM_ROUND_INTERVAL_S | 180   |
      | KRONIKA_DEMO_WORKLOAD_VACUUM_STATEMENT_TIMEOUT_S | 10  |
    And a PostgreSQL writing its log as stderr
    And a PgBouncer in front of it
    And the demo workload reaches PostgreSQL through PgBouncer
    When the demo finishes within 40 seconds
    Then the demo log contains these lines
      | line                                                        |
      | plan story entering sequential-scan regression              |
      | plan story restored the checkout index                      |
      | OLTP workload starting 1 clients at up to 10 transactions/s |
    And PostgreSQL returns these scalar values
      | query                                                                                                                    | value |
      | select (to_regclass('shop.checkout_orders_customer_placed_idx') is not null)::text                                       | true  |
      | select count(*) from pg_stat_activity where datname = current_database() and xact_start < now() - interval '5 seconds'   | 0     |
      | select (count(*) between 1 and 4)::text from shop.orders where id > 30000                                                 | true  |
      | select (count(*) between 1 and 4)::text from shop.orders o join shop.order_items i on i.order_id = o.id join shop.payments p on p.order_id = o.id join shop.event_log e on e.order_id = o.id where o.id > 30000 | true  |
      | select (count(*) > 0)::text from shop.inventory where reserved > 0 and available < 16                                    | true  |
      | select count(*)::text from shop.order_items i left join shop.orders o on o.id = i.order_id where o.id is null             | 0     |

  Scenario: System activity runs without PostgreSQL and removes its scratch file
    Given a demo workload with these settings
      | variable                                      | value |
      | KRONIKA_DEMO_DURATION_S                       | 2     |
      | KRONIKA_INTERVAL_S                            | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES                     | 1     |
      | KRONIKA_DEMO_COLLECTOR_LOG                    | stderr |
      | KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED          | true  |
      | KRONIKA_DEMO_SYSTEM_CPU_PERCENT               | 1     |
      | KRONIKA_DEMO_SYSTEM_MEMORY_MIB                | 8     |
      | KRONIKA_DEMO_SYSTEM_FILE_MIB                  | 1     |
      | KRONIKA_DEMO_SYSTEM_DISK_KIB_PER_S            | 4     |
      | KRONIKA_DEMO_SYSTEM_NETWORK_KIB_PER_S         | 4     |
      | KRONIKA_DEMO_SYSTEM_FLUSH_INTERVAL_S          | 1     |
    When the demo finishes within 20 seconds
    Then the demo log contains these lines
      | line                    |
      | system workload started |
      | system workload stopped |
    And the demo data root lacks these paths
      | path                                             |
      | system-activity/kronika-demo-system-activity.bin |
