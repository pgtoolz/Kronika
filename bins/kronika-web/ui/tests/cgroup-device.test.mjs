import assert from "node:assert/strict"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const devices = await importModule(
  'export { blockParents, cgroupDeviceChain, cgroupDevicePresentation, cgroupDevicePrimary, cgroupDeviceSecondary, exactDeviceId } from "../src/cgroup-device.ts"',
  { plugins: [registryPlugin([])] },
)

const row = (logicalName, ordinal, values) => ({
  logicalName,
  ordinal,
  segmentId: "frozen-zms",
  timestamp: 1_788_523_170_023_568,
  typeId: logicalName,
  values,
})

const edge = (major, minor, parentMajor, parentMinor) => row("os_block_topology", `${major}:${minor}>${parentMajor}:${parentMinor}`, { major, minor, parent_major: parentMajor, parent_minor: parentMinor, scope: 0 })

// The demo container: a Docker volume on the LVM volume data-docker (dm-0)
// over the partition nvme0n1p4 of the disk nvme0n1. io.stat charges dm-0 and
// nvme0n1; the partition in between is neither charged nor mounted.
const mounts = [
  row("os_mountinfo", "data", { major: 252, minor: 0, mount_point: "/var/lib/kronika/data", root: "/volumes/demo_data/_data", source: "/dev/mapper/data-docker", is_k8s_infra: false, scope: 0 }),
  row("os_mountinfo", "hosts", { major: 252, minor: 0, mount_point: "/etc/hosts", root: "/containers/id/hosts", source: "/dev/mapper/data-docker", is_k8s_infra: "true", scope: 0 }),
]
const diskRows = [
  row("os_diskstats", "dm-0", { major: 252, minor: 0, device: "dm-0", scope: 0 }),
  row("os_diskstats", "nvme0n1", { major: 259, minor: 0, device: "nvme0n1", scope: 0 }),
]
const parents = devices.blockParents([edge(252, 0, 259, 4), edge(259, 4, 259, 0), edge(259, 1, 259, 0)])
const charged = ["252:0", "259:0"]
const io = (major, minor) => row("os_cgroup_io", `${major}:${minor}`, { cgroup_path: "/", major, minor, scope: 3 })

test("the mounted layer names the stream and the chain runs down to the disk", () => {
  const top = devices.cgroupDevicePresentation(io(252, 0), mounts, diskRows, parents, charged)
  assert.equal(top.id, "252:0")
  assert.equal(top.device, "dm-0")
  assert.equal(top.foldedInto, null)
  assert.deepEqual(top.preferredMounts, ["/var/lib/kronika/data"])
  assert.deepEqual(top.chain, [{ id: "252:0", name: "dm-0" }, { id: "259:4", name: null }, { id: "259:0", name: "nvme0n1" }])
  assert.equal(devices.cgroupDevicePrimary(top), "/var/lib/kronika/data")
  assert.equal(devices.cgroupDeviceSecondary(top), "data-docker · dm-0 → nvme0n1")
  assert.equal(devices.cgroupDeviceChain(top), "dm-0 252:0 → 259:4 → nvme0n1 259:0")
  assert.equal(top.associations.some(({ mountPoint }) => mountPoint === "/etc/hosts"), true)
})

test("a charged lower layer folds under the charged device above it", () => {
  const disk = devices.cgroupDevicePresentation(io(259, 0), mounts, diskRows, parents, charged)
  assert.equal(disk.foldedInto, "252:0")
  assert.equal(disk.device, "nvme0n1")
  // The volume above owns its mount; the disk does not borrow it.
  assert.deepEqual(disk.preferredMounts, [])
  assert.deepEqual(disk.chain, [{ id: "259:0", name: "nvme0n1" }])
  assert.equal(devices.cgroupDevicePrimary(disk), "nvme0n1")
  assert.equal(devices.cgroupDeviceSecondary(disk), null)
})

test("a disk charged for a partition mount takes the mount above it", () => {
  const partitionMounts = [row("os_mountinfo", "boot", { major: 259, minor: 1, mount_point: "/boot", root: "/", source: "/dev/nvme0n1p1", is_k8s_infra: false, scope: 0 })]
  const named = [...diskRows, row("os_diskstats", "nvme0n1p1", { major: 259, minor: 1, device: "nvme0n1p1", scope: 0 })]
  const disk = devices.cgroupDevicePresentation(io(259, 0), partitionMounts, named, parents, ["259:0"])
  assert.equal(disk.foldedInto, null)
  assert.deepEqual(disk.preferredMounts, ["/boot"])
  assert.deepEqual(disk.associations.map(({ via }) => via), ["259:1"])
  assert.deepEqual(disk.chain.map(({ id }) => id), ["259:1", "259:0"])
  assert.equal(devices.cgroupDevicePrimary(disk), "/boot")
  assert.equal(devices.cgroupDeviceSecondary(disk), "nvme0n1p1 → nvme0n1")
})

test("without topology a device is named by its exact mount, name or bare identity", () => {
  const plain = devices.cgroupDevicePresentation(io(259, 0), [], [], new Map(), ["259:0", "252:0"])
  assert.deepEqual(plain, { associations: [], chain: [{ id: "259:0", name: null }], device: null, foldedInto: null, id: "259:0", preferredMounts: [], source: null })
  assert.equal(devices.cgroupDevicePrimary(plain), "259:0")
  assert.equal(devices.cgroupDeviceSecondary(plain), null)
  assert.equal(devices.cgroupDeviceChain(plain), "259:0")

  const named = devices.cgroupDevicePresentation(io(259, 0), [], diskRows)
  assert.equal(devices.cgroupDevicePrimary(named), "nvme0n1")
  assert.equal(devices.cgroupDeviceSecondary(named), null)
})

test("ambiguous exact disk names stay opaque", () => {
  const presentation = devices.cgroupDevicePresentation(io(7, 0), [], [
    row("os_diskstats", "a", { major: 7, minor: 0, device: "loop0" }),
    row("os_diskstats", "b", { major: 7, minor: 0, device: "loop-other" }),
  ])
  assert.equal(presentation.device, null)
  assert.equal(devices.cgroupDevicePrimary(presentation), "7:0")
  assert.equal(devices.exactDeviceId(io(7, 0)), "7:0")
})
