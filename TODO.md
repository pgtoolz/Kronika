# TODO

## Collector: one worker thread for `statvfs`

- [ ] Consider replacing the child process with one persistent worker thread.
  Deferred; keep the [process implementation](bins/kronika-collector/src/filesystem_capacity.rs).

`statvfs` can block indefinitely when storage stops responding, for example on a
hard NFS mount. NFS is excluded by the current allowlist, but an allowed filesystem
can still use unresponsive backing storage. A timeout does not interrupt the
system call, and Rust cannot safely terminate its thread. The child process keeps
the blocked call outside the collector and allows a separate termination request.
The kernel can delay termination even after `SIGKILL`.

Thread alternative: allow one pass at a time, with no queue or replacement threads
on timeout. Keep completed results and leave missing values `null`. Skip new passes
while the worker is blocked. Never report late results as samples from a later pass.

Before switching, decide whether losing capacity metrics until the call returns
or the collector restarts is acceptable. Test storage failure and recovery, and
shutdown without an unbounded `join`. If accepted, remove self-exec, `memfd`, and
the binary protocol.
