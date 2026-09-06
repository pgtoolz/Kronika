Feature: A journal the collector cannot read

  The collector refuses to replace or publish a journal it cannot process.
  The bytes stay at active.wal so an operator can inspect them or retry with a
  compatible collector.

  Scenario: A journal that will not open remains canonical
    Given a data root whose active.wal is the journal magic followed by "not-a-valid-header"
    And a collector with these settings
      | variable                  | value |
      | KRONIKA_INTERVAL_S        | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES | 1     |
    When it runs for 3 seconds
    Then the log reports that active.wal was preserved
    And the raw journal is named active.wal
    And no segment exists under a YYYY/MM/DD directory

  Scenario: A torn populated journal is not partially published
    Given a collector with these settings
      | variable                    | value      |
      | KRONIKA_INTERVAL_S          | 1          |
      | KRONIKA_SEGMENT_MAX_BYTES   | 1073741824 |
      | KRONIKA_SEGMENT_MAX_AGE_S   | 86400      |
      | KRONIKA_OS_CORE_INTERVAL_S  | 0          |
    When it runs for 4 seconds
    And the last 200 bytes of the journal are cut off
    And it starts again over the same data root and runs for 3 seconds
    Then the log reports that active.wal was preserved
    And the raw journal is named active.wal
    And no segment exists under a YYYY/MM/DD directory
