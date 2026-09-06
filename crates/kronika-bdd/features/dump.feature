Feature: Reading a data directory back with the dumper

  The dumper is the first consumer of the reader outside the tests. What it
  prints is what a dashboard would be served, so a scenario asserts on its
  output rather than on a helper written for the suite.

  Scenario: The dumper explains what a segment is made of
    Given a collector with these settings
      | variable                  | value |
      | KRONIKA_INTERVAL_S        | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES | 1     |
    When it runs for 3 seconds
    Then the dumper reports these sections with a size
      | section           |
      | instance_metadata |
      | os_process        |
      | os_meminfo        |
      | os_psi            |
    And the dumper prints every section byte count as a share of the segment

  Scenario: The dumper prints rows, with dictionary ids resolved
    Given a collector with these settings
      | variable                  | value |
      | KRONIKA_INTERVAL_S        | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES | 1     |
    When it runs for 3 seconds
    Then the dumper prints the rows of section 1107001
    And column mount_point of section 1112002 starts with / rather than an id

  Scenario: The dumper builds a health point for every pressure snapshot
    Given a collector with these settings
      | variable                   | value      |
      | KRONIKA_INTERVAL_S         | 1          |
      | KRONIKA_SEGMENT_MAX_BYTES  | 1073741824 |
      | KRONIKA_SEGMENT_MAX_AGE_S  | 3          |
    When it runs for 6 seconds
    Then the dumper builds one OS health point per pressure snapshot
    And every health series point is null or between 0 and 100
