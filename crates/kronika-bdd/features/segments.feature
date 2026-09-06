Feature: Where a segment lands and what it says about itself

  Every scenario runs the real binary. A scenario carries its own settings, so
  the behaviour under test is readable without opening a step definition.

  Scenario: A finished segment lands on its UTC calendar path
    Given a collector with these settings
      | variable                  | value |
      | KRONIKA_INTERVAL_S        | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES | 1     |
    When it runs for 3 seconds
    Then a segment exists under a YYYY/MM/DD directory
    And every published segment file ends in .zms
    And the raw journal is named active.wal

  Scenario: Writing reports where the segment went and what it cost
    Given a collector with these settings
      | variable                  | value |
      | KRONIKA_INTERVAL_S        | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES | 1     |
    When it runs for 3 seconds
    Then the log has a segment_write_finish line naming these fields
      | field         |
      | segment_path  |
      | segment_id    |
      | reason        |
      | sections      |
      | segment_bytes |
      | journal_bytes |
      | journal_parts |
      | min_ts        |
      | max_ts        |
      | elapsed_ms    |
      | rss_kib       |

  Scenario: An open segment closes on age and carries every window it collected
    Given a collector with these settings
      | variable                    | value      |
      | KRONIKA_INTERVAL_S          | 1          |
      | KRONIKA_SEGMENT_MAX_BYTES   | 1073741824 |
      | KRONIKA_SEGMENT_MAX_AGE_S   | 3          |
      | KRONIKA_OS_CORE_INTERVAL_S  | 0          |
    When it runs for 6 seconds
    Then every segment covers at least 3 windows
    And every segment ends later than it starts

  Scenario: Stopping the collector leaves the journal in place
    Given a collector with these settings
      | variable                  | value      |
      | KRONIKA_INTERVAL_S        | 1          |
      | KRONIKA_SEGMENT_MAX_BYTES | 1073741824 |
      | KRONIKA_SEGMENT_MAX_AGE_S | 86400      |
      | KRONIKA_OS_CORE_INTERVAL_S | 0          |
    When it runs for 3 seconds
    Then the raw journal is named active.wal
    And the log has no error line

  Scenario: A time range selects only the segments it overlaps
    Given a collector with these settings
      | variable                  | value |
      | KRONIKA_INTERVAL_S        | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES | 1     |
    When it runs for 4 seconds
    Then at least 2 segments were published
    And reading each segment's own window returns that segment
    And reading the first segment's window leaves out the last segment
    And reading the time before the first segment returns nothing
    And reading the time after the last segment returns nothing

  Scenario: A segment the reader will not admit is set aside
    Given a collector with these settings
      | variable                  | value |
      | KRONIKA_INTERVAL_S        | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES | 1     |
    When it runs for 4 seconds
    And the oldest published segment is cut down to 200 bytes
    Then the reader sets aside 1 file
