Feature: Container pressure and storage reach the recorded segment

  Background:
    Given a filesystem fixture with these entries
      | kind      | path                                  | value                                |
      | file line | proc/stat                             | cpu 10 0 5 100 0 0 0 0 0 0           |
      | file line | proc/stat                             | cpu0 10 0 5 100 0 0 0 0 0 0          |
      | file line | proc/stat                             | btime 1700000000                     |
      | file line | proc/sys/kernel/hostname              | container-fixture                    |
      | file line | proc/sys/kernel/osrelease             | 6.8.0-fixture                        |
      | file line | proc/sys/kernel/random/boot_id        | 11111111-2222-4333-8444-555555555555 |
      | file line | proc/1/cgroup                         | 0::/docker/workload                  |
      | file line | proc/self/cgroup                      | 0::/docker/workload                  |
      | file line | sys/fs/cgroup/cgroup.controllers      | cpu memory io pids                   |
      | file line | sys/fs/cgroup/docker/workload/io.stat |                                      |

  Scenario: PSI stores the exact container membership with scope 3
    Given a filesystem fixture with these entries
      | kind      | path                                                | value                                               |
      | file line | sys/fs/cgroup/docker/workload/cpu.pressure          | some avg10=1.5 avg60=1.0 avg300=0.5 total=11000     |
      | file line | sys/fs/cgroup/docker/workload/memory.pressure       | some avg10=2.5 avg60=2.0 avg300=1.5 total=22000     |
      | file line | sys/fs/cgroup/docker/workload/memory.pressure       | full avg10=0.5 avg60=0.25 avg300=0.125 total=2200   |
      | file line | sys/fs/cgroup/docker/workload/io.pressure           | some avg10=3.5 avg60=3.0 avg300=2.5 total=33000     |
      | file line | sys/fs/cgroup/docker/workload/io.pressure           | full avg10=1.5 avg60=1.0 avg300=0.5 total=3300      |
      | file line | proc/pressure/cpu                                   | some avg10=91.0 avg60=90.0 avg300=89.0 total=910000 |
      | file line | proc/pressure/memory                                | some avg10=92.0 avg60=91.0 avg300=90.0 total=920000 |
      | file line | proc/pressure/memory                                | full avg10=81.0 avg60=80.0 avg300=79.0 total=820000 |
      | file line | proc/pressure/io                                    | some avg10=93.0 avg60=92.0 avg300=91.0 total=930000 |
      | file line | proc/pressure/io                                    | full avg10=82.0 avg60=81.0 avg300=80.0 total=830000 |
      | file line | sys/fs/cgroup/docker/cpu.pressure                   | some avg10=71.0 avg60=70.0 avg300=69.0 total=710000 |
      | file line | sys/fs/cgroup/docker/memory.pressure                | some avg10=72.0 avg60=71.0 avg300=70.0 total=720000 |
      | file line | sys/fs/cgroup/docker/memory.pressure                | full avg10=61.0 avg60=60.0 avg300=59.0 total=620000 |
      | file line | sys/fs/cgroup/docker/io.pressure                    | some avg10=73.0 avg60=72.0 avg300=71.0 total=730000 |
      | file line | sys/fs/cgroup/docker/io.pressure                    | full avg10=62.0 avg60=61.0 avg300=60.0 total=630000 |
      | file line | sys/fs/cgroup/docker/workload/child/cpu.pressure    | some avg10=51.0 avg60=50.0 avg300=49.0 total=510000 |
      | file line | sys/fs/cgroup/docker/workload/child/memory.pressure | some avg10=52.0 avg60=51.0 avg300=50.0 total=520000 |
      | file line | sys/fs/cgroup/docker/workload/child/memory.pressure | full avg10=41.0 avg60=40.0 avg300=39.0 total=420000 |
      | file line | sys/fs/cgroup/docker/workload/child/io.pressure     | some avg10=53.0 avg60=52.0 avg300=51.0 total=530000 |
      | file line | sys/fs/cgroup/docker/workload/child/io.pressure     | full avg10=42.0 avg60=41.0 avg300=40.0 total=430000 |
    And a collector with these settings
      | variable                   | value          |
      | KRONIKA_PROC_ROOT          | {fixture}/proc |
      | KRONIKA_SYS_ROOT           | {fixture}/sys  |
      | KRONIKA_INTERVAL_S         | 1              |
      | KRONIKA_SEGMENT_MAX_BYTES  | 1              |
      | KRONIKA_OS_CORE_INTERVAL_S | 0              |
    When it runs for 3 seconds
    Then every segment records these instance facts
      | column      | value |
      | environment | 1     |
    And every segment holds these sections
      | type_id | section | min rows |
      | 1107001 | os_psi  | 3        |
    And every snapshot of section 1107001 contains exactly these rows
      | resource | scope | some_avg10 | some_total | full_total |
      | 0        | 3     | 1.5        | 11000      | null       |
      | 1        | 3     | 2.5        | 22000      | 2200       |
      | 2        | 3     | 3.5        | 33000      | 3300       |
    And no segment records these rows
      | type_id | column     | value  |
      | 1107001 | scope      | 0      |
      | 1107001 | some_total | 910000 |
      | 1107001 | some_total | 920000 |
      | 1107001 | some_total | 930000 |
      | 1107001 | some_total | 710000 |
      | 1107001 | some_total | 720000 |
      | 1107001 | some_total | 730000 |
      | 1107001 | some_total | 510000 |
      | 1107001 | some_total | 520000 |
      | 1107001 | some_total | 530000 |

  Scenario: Storage keeps mounted and charged devices and excludes host siblings and kernel masks
    Given a filesystem fixture with these entries
      | kind      | path                                       | value                                                 |
      | file line | proc/self/mountinfo                        | 30 1 252:0 / /tmp rw - ext4 /dev/dm-0 rw              |
      | file line | proc/self/mountinfo                        | 31 1 0:60 / /var/tmp rw - tmpfs tmpfs rw              |
      | file line | proc/self/mountinfo                        | 32 1 0:61 / /procdata rw - tmpfs tmpfs rw             |
      | file line | proc/self/mountinfo                        | 33 1 0:62 / /sysdata rw - tmpfs tmpfs rw              |
      | file line | proc/self/mountinfo                        | 34 1 8:17 /hosts /etc/hosts rw - ext4 /dev/sdb1 rw    |
      | file line | proc/self/mountinfo                        | 40 1 0:70 / /proc rw - tmpfs tmpfs rw                 |
      | file line | proc/self/mountinfo                        | 41 40 0:71 / /proc/irq rw - tmpfs tmpfs rw            |
      | file line | proc/self/mountinfo                        | 42 40 0:72 / /proc/kcore rw - tmpfs tmpfs rw          |
      | file line | proc/self/mountinfo                        | 43 40 8:17 / /proc/masked-disk rw - ext4 /dev/sdb1 rw |
      | file line | proc/self/mountinfo                        | 44 1 0:73 / /sys rw - tmpfs tmpfs rw                  |
      | file line | proc/self/mountinfo                        | 45 44 0:74 / /sys/firmware rw - tmpfs tmpfs rw        |
      | file line | proc/diskstats                             | 252 0 dm-0 11 0 110 1 21 0 210 2 0 3 4                |
      | file line | proc/diskstats                             | 8 0 sda 31 0 310 1 41 0 410 2 0 3 4                   |
      | file line | proc/diskstats                             | 252 2 dm-2 51 0 510 1 61 0 610 2 0 3 4                |
      | file line | proc/diskstats                             | 8 1 sda1 71 0 710 1 81 0 810 2 0 3 4                  |
      | file line | proc/diskstats                             | 252 9 dm-9 901 0 9010 1 911 0 9110 2 0 3 4            |
      | file line | proc/diskstats                             | 8 16 sdb 921 0 9210 1 931 0 9310 2 0 3 4              |
      | file line | proc/diskstats                             | 8 17 sdb1 941 0 9410 1 951 0 9510 2 0 3 4             |
      | file line | sys/fs/cgroup/docker/workload/io.stat      | 8:0 rbytes=4096 wbytes=8192 rios=1 wios=2             |
      | file line | sys/fs/cgroup/docker/workload/io.stat      | 252:2 rbytes=12288 wbytes=16384 rios=3 wios=4         |
      | file line | sys/fs/cgroup/docker/io.stat               | 252:9 rbytes=999999 wbytes=999999 rios=99 wios=99     |
      | file line | sys/fs/cgroup/docker/io.stat               | 8:17 rbytes=999999 wbytes=999999 rios=99 wios=99      |
      | file line | sys/devices/virtual/block/dm-0/dev         | 252:0                                                 |
      | file line | sys/devices/virtual/block/dm-2/dev         | 252:2                                                 |
      | file line | sys/devices/virtual/block/dm-9/dev         | 252:9                                                 |
      | file line | sys/devices/block/sda/dev                  | 8:0                                                   |
      | file line | sys/devices/block/sda/sda1/dev             | 8:1                                                   |
      | file line | sys/devices/block/sda/sda1/partition       | 1                                                     |
      | file line | sys/devices/block/sdb/dev                  | 8:16                                                  |
      | file line | sys/devices/block/sdb/sdb1/dev             | 8:17                                                  |
      | file line | sys/devices/block/sdb/sdb1/partition       | 1                                                     |
      | symlink   | sys/dev/block/252:0                        | ../../devices/virtual/block/dm-0                      |
      | symlink   | sys/dev/block/252:2                        | ../../devices/virtual/block/dm-2                      |
      | symlink   | sys/dev/block/252:9                        | ../../devices/virtual/block/dm-9                      |
      | symlink   | sys/dev/block/8:0                          | ../../devices/block/sda                               |
      | symlink   | sys/dev/block/8:1                          | ../../devices/block/sda/sda1                          |
      | symlink   | sys/dev/block/8:16                         | ../../devices/block/sdb                               |
      | symlink   | sys/dev/block/8:17                         | ../../devices/block/sdb/sdb1                          |
      | symlink   | sys/devices/virtual/block/dm-0/slaves/sda1 | ../../../../block/sda/sda1                            |
      | symlink   | sys/devices/virtual/block/dm-2/slaves/sda  | ../../../../block/sda                                 |
      | symlink   | sys/devices/virtual/block/dm-9/slaves/sda  | ../../../../block/sda                                 |
    And a collector with these settings
      | variable                        | value          |
      | KRONIKA_PROC_ROOT               | {fixture}/proc |
      | KRONIKA_SYS_ROOT                | {fixture}/sys  |
      | KRONIKA_INTERVAL_S              | 1              |
      | KRONIKA_SEGMENT_MAX_BYTES       | 1              |
      | KRONIKA_OS_CORE_INTERVAL_S      | 0              |
      | KRONIKA_OS_MOUNTTOPO_INTERVAL_S | 0              |
    When it runs for 3 seconds
    Then every segment records these instance facts
      | column      | value |
      | environment | 1     |
    And every segment holds these sections
      | type_id | section           | min rows |
      | 1108001 | os_diskstats      | 3        |
      | 1123001 | os_block_topology | 3        |
      | 1112002 | os_mountinfo      | 5        |
    And every snapshot of section 1108001 contains exactly these rows
      | major | minor | device | scope | reads | writes | read_sectors | write_sectors |
      | 252   | 0     | dm-0   | 0     | 11    | 21     | 110          | 210           |
      | 8     | 0     | sda    | 0     | 31    | 41     | 310          | 410           |
      | 252   | 2     | dm-2   | 0     | 51    | 61     | 510          | 610           |
    And every snapshot of section 1123001 contains exactly these rows
      | major | minor | parent_major | parent_minor | scope |
      | 252   | 0     | 8            | 1            | 0     |
      | 8     | 1     | 8            | 0            | 0     |
      | 252   | 2     | 8            | 0            | 0     |
    And every snapshot of section 1112002 contains exactly these rows
      | major | minor | mount_point | root   | fstype | source    | is_k8s_infra | scope |
      | 252   | 0     | /tmp        | /      | ext4   | /dev/dm-0 | false        | 0     |
      | 0     | 60    | /var/tmp    | /      | tmpfs  | tmpfs     | false        | 0     |
      | 0     | 61    | /procdata   | /      | tmpfs  | tmpfs     | false        | 0     |
      | 0     | 62    | /sysdata    | /      | tmpfs  | tmpfs     | false        | 0     |
      | 8     | 17    | /etc/hosts  | /hosts | ext4   | /dev/sdb1 | true         | 0     |
    And no segment records these rows
      | type_id | column       | value             |
      | 1108001 | device       | sda1              |
      | 1108001 | device       | dm-9              |
      | 1108001 | device       | sdb               |
      | 1108001 | device       | sdb1              |
      | 1123001 | minor        | 9                 |
      | 1123001 | minor        | 17                |
      | 1123001 | parent_minor | 16                |
      | 1112002 | mount_point  | /proc             |
      | 1112002 | mount_point  | /proc/irq         |
      | 1112002 | mount_point  | /proc/kcore       |
      | 1112002 | mount_point  | /proc/masked-disk |
      | 1112002 | mount_point  | /sys              |
      | 1112002 | mount_point  | /sys/firmware     |
