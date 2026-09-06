Feature: PostgreSQL monitoring sessions stop waiting for conflicting locks

  Scenario: Index metadata contention leaves the same monitoring session usable
    Given a PostgreSQL writing its log as stderr
    Then the monitoring index read rejects the lock wait and recovers on the same session
      | setting            | value |
      | setup SQL          | CREATE TABLE bdd_locked_index (id integer PRIMARY KEY) |
      | lock SQL           | BEGIN; LOCK TABLE bdd_locked_index IN ACCESS EXCLUSIVE MODE |
      | release SQL        | ROLLBACK |
      | expected index     | bdd_locked_index_pkey |
      | SQLSTATE           | 55P03 |
      | maximum wait ms    | 5000 |
      | statement timeout  | 30s |
      | lock timeout       | 100ms |
      | unrelated SQL      | SELECT 1 |
      | unrelated result   | 1 |
      | backend state      | idle |
      | waiting locks      | 0 |
      | open transactions  | 0 |
