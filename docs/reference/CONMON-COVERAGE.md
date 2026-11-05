# Conmon responsibility coverage and revised requirements

Revised 2026-11-05 after the user required multiprocess supervision, evaluation
of TTY support, conmon logging parity, notification relay, runtime deadlines,
and the previously identified cleanup, placement, OOM, and durability functions.
This supersedes the exclusions in the initial audit and simplification review.
Madrillo is still an architecture proposal, not an implemented monitor.

The comparison uses C conmon revision
`46837cd4800889521b6eccd4cce75a69249b1702` and Podman revision
`8d37a39bb1cc07a1425996a8adf24be9213d2f4c`. The inspected files were checked
against pinned upstream sources. The separate conmon-v3 rewrite is not the
reference. Source compatibility means the relevant behaviors and formats,
not copying bugs, unsafe code, every flag alias, or OCI-specific machinery.

## Revised responsibility matrix

| Function | Revised madrillo requirement | Boundary |
| --- | --- | --- |
| Independent monitor lifetime, attach/detach, input ownership | Required | One monitor per workload, independent of pichi attachments |
| Native launch and retained exit result | Required | Leader status and workload cleanup are separate facts |
| Descendant supervision, reaping, workload termination | Required | Linux cgroup/subreaper, Windows job; macOS containment remains unresolved |
| Host terminal support and resize | Evaluate for inclusion; recommended opt-in | PTY/ConPTY for host programs; dillo VM console stays on pipes |
| Conmon logging formats and destinations | Required | CRI file, journald, none, passthrough, supported combinations |
| Limits, rotation/truncation, reopen, labels/tags | Required where applicable | Do not replace these with a proprietary transcript or a drop-on-full default |
| Shutdown log synchronization | Required by default | Explicit opt-out; errors and crash limits remain visible |
| Standard service notification relay | Required where supported | Filtered sd_notify, not a child-specific management proxy |
| Systemd activation descriptor/PID adaptation | Include with service integration | Passing raw descriptors alone is insufficient; safe spawn support needs proof |
| Monitor-owned runtime timeout | Required | Monotonic hard deadline, independent of pichi lifetime |
| Monitor-held resources and exit command | Required | Generic lifetime mechanisms; pichi owns cleanup/restart policy |
| Resource placement before execution | Required | Ordering and actual atomic/native assignment are separate requirements |
| Linux OOM observation and monitor protection | Required where available | Report missing permissions/evidence; do not invent an equivalent on other hosts |
| Diagnostics, endpoint protection, bounded input | Required | Operational failures do not panic; private instance resources |
| OCI bundles/runtime create/exec/restore adapter | Excluded | Direct executable launch; dillo-specific operations remain its contract |
| Guest command execution, VM management proxy | Excluded | Remain direct grandparent/grandchild contracts |
| Engine image management and restart decisions | Outside madrillo | An exit command may invoke caller policy without embedding it |

The source inventory is conmon's [CLI][cli], [main lifecycle][main],
[runtime invocation][runtime], [attachment/notify I/O][sockets], [terminal
control][control], [logging][logging], [exit handling][exit], [cgroup events][cgroup],
[OOM adjustment][oom], and [descriptor ownership][fds].

## A workload can contain multiple processes

The original direct-child-only contract was too narrow. Madrillo launches a
leader, but supervises its workload scope, including helpers. Killing or timing
out only the leader does not satisfy this requirement. Leader exit initiates
workload finalization; retain its exit status while cleaning up surviving
helpers. Do not report successful cleanup while the supported scope remains
populated. Failure to finish cleanup is a distinct outcome, not an invented
successful exit. A backgrounding launcher cannot silently redefine which
process supplies the workload's primary result.

Conmon's subreaper adopts orphaned descendants so it can wait for them. It is
not a complete containment mechanism. Its normal environment also includes an
OCI runtime, cgroups, and commonly a PID namespace whose init exiting causes
remaining namespace processes to be killed. Conmon's timeout code itself sends
SIGKILL to a process group when possible. Do not transfer container guarantees
to ordinary host children merely by copying that call. [Subreaper][subreaper],
[conmon exit/reaping][exit], [PID namespace lifetime][pid-namespaces], [timeout][main].

| Host | Direction | Qualification still required |
| --- | --- | --- |
| Linux | Dedicated workload cgroup, monitor outside its limits, subreaper for orphan wait/reap | Delegation/permissions, child membership before execution, all-descendant termination and empty-scope observation; cgroup v1/v2 support cannot be silently conflated |
| Windows | Job Object containing the workload and descendants, with ordinary breakaway disabled | Membership before execution, nested-job restrictions, completion tracking, safe API composition with ConPTY |
| macOS | Native process groups/sessions for cooperative workloads; process events for observation | This is not equivalent to cgroup/job containment. A child can change groups/sessions; terminal shells create job-control groups. Arbitrary descendant cleanup has no qualified solution in this design yet |

Linux cgroup v2 supplies subtree population and termination facilities; Windows
jobs normally include newly created children. Neither makes pichi's requested
resource policy disappear. Policy belongs to pichi; madrillo owns enforcement
of the supplied lifetime scope. Unsupported required containment must be reported
rather than silently falling back to killing a single PID. [Linux cgroups][cgroup-v2],
[Windows jobs][windows-jobs], [Apple process groups][apple-pgroups].

The macOS limitation is an open architecture issue, not permission to drop
multiprocess dillo. A cooperative dillo can keep helpers under its ownership;
that contract and abnormal-exit behavior need proof. Periodically enumerating
PIDs, or observing fork/exit events, is not a race-free substitute for containment.
Do not claim that all arbitrary daemonizing host programs are covered.

Monitor-loss cleanup remains a separate requirement. Dillo detects loss of its
retained console input and coordinates helper/device cleanup. An immediate
Windows kill-on-last-job-handle-close must not preempt that cleanup. A dead
monitor cannot run a callback, reap, or enforce an escalation timer. Resolving
job ownership and dillo failure behavior is part of native qualification.

## Evaluate a real terminal mode

Recommendation: include an explicit `--terminal` mode, with initial size and
resize support, alongside ordinary pipes. The monitor owns the host terminal
for the workload lifetime; attachments only copy bytes and request resize.
Dropping an attachment never destroys the terminal. Pichi still owns local
raw mode, detach keys, and restoration. TERM/environment are supplied by the
caller, not inferred as VM knowledge.

| Behavior | Pipes | Host terminal |
| --- | --- | --- |
| Typical use | Dillo console, binary streams, noninteractive commands | Shells, job control, interactive host programs that test for a terminal |
| Host implementation | Process stdin/stdout/stderr pipes | PTY on Linux/macOS, ConPTY on Windows |
| Output identity | Separate stdout/stderr | Normally one combined terminal stream |
| Input interpretation | Bytes to child | Unix terminal line discipline or Windows console processing applies |
| Resize | No generic pipe resize | Resize the native terminal; terminal-aware child observes the change |
| EOF | Pipe writer can close independently | No equivalent portable stdin-only half-close; EOF characters are terminal data |
| Detach | Keep owned pipes | Keep terminal and I/O servicing alive |

Conmon receives a PTY master from the OCI runtime and applies resize through
its control path. Without an OCI runtime, madrillo must create the terminal and
spawn the child with the correct native session/controlling-terminal setup.
It should not implement a terminal emulator. [Conmon terminal ownership][control].

Dillo should continue to receive pipe-backed console bytes: its guest already
supplies terminal semantics. A second host line discipline may echo, transform
bytes, or intercept control characters, and would complicate the console-loss
cleanup contract. Guest resize remains pichi-to-dillo; host-terminal resize is
a separate generic monitor operation.

ConPTY produces UTF-8 terminal sequences and has platform-specific teardown
behavior. Closing it can notify/terminate attached console clients; output must
be drained correctly to avoid deadlock on supported older Windows versions.
Do not model it as three ordinary Unix file descriptors or promise arbitrary
binary transparency. [ConPTY creation][conpty-create], [encoding][conpty],
[teardown][conpty-close].

`portable-pty` exposes safe allocation, spawn, read/write, and resize APIs on
these platforms and is a candidate, not a qualified dependency. Check native
argument fidelity, exit-status fidelity, ownership/drop behavior, inherited
resource allowlists, and composition with cgroup/job placement. `process-wrap`
is another candidate for process lifetime primitives, but its own documentation
notes PTY composition concerns. Neither candidate proves all requirements.
No madrillo-owned unsafe shim is allowed. [portable-pty][portable-pty],
[process-wrap][process-wrap].

## Match conmon logging

The prior custom binary transcript and unconditional drop-on-full queue policy
are withdrawn. Implement conmon's concrete logging functions in private binary
modules, without a plugin framework or public logger API.

- **File format:** CRI/Kubernetes records with timestamp, stdout/stderr, and
  partial/full markers. Preserve payload bytes; do not force UTF-8 conversion
  for pipe data. Terminal output is a combined stream. Chunk boundaries can
  vary with reads; reconstructing partial records must preserve the payload.
- **Destinations:** a file, journald, both where supported, `none`, and native
  stdio passthrough. Journald availability is explicit on hosts without it;
  no emulated journal is introduced. Tags/labels and partial-message metadata
  follow the driver that supports them.
- **Retention controls:** per-file size, global output cap, rotation with backup
  count, and conmon's truncation behavior when rotation is disabled. In the
  pinned source, global size is a cumulative writing cap, not merely current
  disk usage after deleting old segments. Do not silently change that meaning.
- **External rotation:** accept a reopen operation over the existing control
  connection. Another Unix-only control FIFO is unnecessary.
- **Shutdown:** flush buffered partial records and synchronize file logs by
  default, with an explicit `--no-sync-log` equivalent. A sync failure is visible.
- **Pressure/errors:** conmon writes synchronously and may backpressure a slow
  sink; write failures are reported and output can be lost. A full queue alone
  must not silently select a different default loss policy. Bound buffers and
  isolate blocked sinks from timeout/control handling. Any deliberate behavioral
  departure must be documented and validated against the requirement.
- **Paths/metadata:** protect configured paths and reopen/rotation operations;
  support the required allowlist behavior when accepting external destinations.
  Caller-supplied workload ID/name map to documented compatibility metadata.
- **Additional captured sources:** keep CRI's stdout/stderr vocabulary. Use
  separate source-named files for UART or other extra streams, and source
  metadata for journal entries. Do not put a new `uart` stream token into a
  supposedly compatible CRI file.

These behaviors come from the [driver and writer implementation][logging] and
[CLI options][cli]. Conmon supports file-plus-journal; this does not call for an
arbitrary collection of dynamically loaded sinks.

The corresponding CLI options include `--log-path`, `--log-size-max`,
`--log-global-size-max`, `--log-rotate`, `--log-max-files`, `--log-tag`,
`--log-label`, `--no-container-partial-message`, `--log-allowlist-dir`, and
`--no-sync-log`. Exact spelling need not preserve deprecated aliases, but the
behaviors are in scope. Parse a recognized driver prefix without splitting a
Windows drive-letter path at every colon.

`none` disables retention but can still permit live attachment. Passthrough
hands native stdio resources directly to the workload; it does not secretly
capture them to preserve reattachment. To support it, move the initial framed
connection from madrillo stdin/stdout to one explicitly inherited connected
channel (`--control HANDLE`). Use a Unix socketpair or a connected Windows duplex
named pipe, with safe ownership qualified as for other inherited resources.
The same channel supplies initial attachment and any placement gate; it does
not add a second bootstrap protocol. Native stdio is then free for passthrough,
and monitor diagnostics remain separate. Managed dillo uses captured pipes,
so this does not change its console-loss contract. A caller-supplied passthrough
pipe/terminal has the lifetime of its actual owner; it does not acquire captured
mode's reconnect guarantees.

Monitor diagnostics are separate from workload logs. Preserve conmon-equivalent
log levels and native syslog output where supported; a host without syslog does
not need an emulation layer.

Pichi reads the selected persistent destination, including journal queries when
configured. No history exists with `none` or passthrough. Retain live attachment
without a replay backlog; files/journal remain the history source. Slow-viewer
isolation is separate from log-sink pressure.

## Service notification and activation

`sd_notify` is systemd's standard notification protocol. A service sends messages
such as `READY=1`, `STATUS=Loading`, `STOPPING=1`, or `WATCHDOG=1` to the socket
specified by `NOTIFY_SOCKET`. A watchdog message tells systemd the service is
still responsive; relaying it does not make madrillo a watchdog. [Systemd
notification contract][sd-notify].

Conmon creates an input socket and forwards selected fields to the host's
notification socket. Its allowlist includes READY, RELOADING, STOPPING,
WATCHDOG, STATUS, ERRNO, BUSERROR, and MONOTONIC_USEC in the accepted forms.
It does not forward arbitrary properties, MAINPID, or descriptor-store requests.
Mirror this restricted relay rather than an unrestricted proxy. Bound datagrams
and preserve message semantics. [Conmon relay/filter][sockets].

Madrillo can provide the same optional standard facility: establish the private
input socket before launch, supply its address through the standard child
environment, and relay accepted messages to the configured service manager.
The sender must have valid service attribution and NotifyAccess permissions;
a relay is not a way around those checks. Do not promise arbitrary systemd
unit arrangements without integration tests. Where the child can notify the
manager directly, the relay can be disabled. Never synthesize readiness from
spawn success or generate heartbeats after the child stops sending them.

This remains generic service integration. Madrillo still does not understand
dillo management APIs, communicate with the VM guest, or proxy build traffic.
A guest notification bridge, if ever required, belongs to dillo/pichi. Ordinary
macOS/Windows launches have no systemd dependency; an unsupported requested
integration must produce a clear error.

Conmon also preserves activation descriptors and adjusts LISTEN_PID for the
runtime child. Equivalent direct-child activation requires correct descriptor
numbers, LISTEN_FDS/LISTEN_FDNAMES, and the child's actual PID. Qualify a safe
spawn mechanism; simply forwarding pichi's environment is wrong. This cannot
be solved by pretending that an argument containing a handle transfers it.
[Conmon environment setup][main], [descriptor setup][fds].

## Runtime deadline

Include `--timeout SECONDS`; unset/zero means no deadline. Conmon's timeout is
a hard kill, not a graceful shutdown protocol. Its timer begins after obtaining
the OCI-created process PID; madrillo's corresponding timer starts at successful
native workload launch. Use a monotonic clock, keep the timer in the monitor,
and record timeout separately from the eventual native exit result. Force
termination targets the supported workload scope, including descendants.
[Conmon timeout setup and termination][main].

The deadline remains active with zero attachments and after pichi exits. It is
separate from startup failure, log-drain limits, guest readiness, and dillo's
optional graceful shutdown API. A hard deadline does not promise guest shutdown
or lossless final output. No polling daemon or new protocol is needed.

## Placement, cleanup, OOM, and durability

**Placement before execution is required.** Conmon initially waits on
`_OCI_STARTPIPE` so Podman can place it before it creates children. Linux moving
a process later does not automatically move already-created descendants.
Windows likewise needs job assignment before the child can spawn escaping
helpers. Keep the monitor outside workload limits. [Podman placement][podman-placement],
[Linux membership][cgroup-v2], [Windows assignment][windows-assignment].

The preconnected initial attachment solves the output race only. Where placement needs an
external manager action, use a minimal `Prepared` / `Start` exchange on the
existing initial connection (`--start-gated`). EOF before Start cancels the
unlaunched workload; after Start it only detaches. The gate orders manager work;
it does not itself implement child cgroup/job assignment. Native creation must
also establish that assignment before workload code runs, using suspended or
atomic creation if required through qualified safe APIs. No additional replay,
bootstrap service, or generic transaction protocol follows from this requirement.

**Retain resources and support an exit command.** Add a monitor-only `--hold`
resource role alongside capture and pass-to-child. Release held reservations
before final exit notification, following conmon. Include one `--exit-command`
with literal repeated arguments and optional delay. It is a native executable,
not a shell string, hook registry, or embedded pichi implementation.
[Conmon held resources][main], [exit helper][exit].

The proposed finalization order is workload cleanup, bounded output drain and
log sync, held-resource release and transient-endpoint cleanup, retained leader
result with cleanup/sync evidence, final notification, then exit-command launch.
A hook failure must not overwrite the workload's native exit result. Pichi's
idempotent cleanup command owns external side effects and records their outcome
in pichi state; restart policy also belongs there if later required. Prevent
workload descriptors and monitor OOM protection from leaking into the helper.
Podman similarly supplies its own engine command. [Podman helper configuration][podman-runtime].

A callback cannot run after monitor SIGKILL or host loss. No exactly-once callback
guarantee is possible from this mechanism alone. Pichi must reconcile incomplete
cleanup on later inspection; native handle-owned resources should still release
automatically. This matches the limits of the conmon mechanism without adding
another always-running supervisor.

**Observe OOM and protect the monitor where possible.** On Linux, monitor the
workload cgroup's memory events and retain the OOM evidence, checking final
counters as well as notifications. Conmon supports cgroup v1/v2 and attempts
`oom_score_adj=-1000`, restoring the inherited adjustment for its runtime and
exit helper. Mirror these functions where permissions permit; report unavailable
protection or observation explicitly. [Conmon cgroup handling][cgroup],
[OOM score handling][oom].

Distinguish a workload OOM event from proof that a particular leader died from
OOM. SIGKILL alone is insufficient. Native memory-limit notifications on other
hosts may provide different evidence; they are not automatically equivalent
to Linux OOM events. The result needs observed/unknown/unavailable distinctions.
Never infer host process limits from the VM's requested RAM/vCPU settings.

**Synchronize logs by default.** Match conmon's shutdown file synchronization;
retain explicit opt-out and failure reporting. Publish the native result
atomically and specify native file/directory flushing where durability is
claimed. File synchronization at shutdown does not promise all earlier records
survive a monitor kill or power loss, and an external journal has its own
persistence contract. Do not label a deadline-aborted sync successful.
[Conmon file synchronization][logging].

## Qualification and simplicity constraints

All implementation remains in the executable. The library shares only the
necessary wire/result types and serialization. Every owned target retains
`forbid(unsafe_code)`. Operational errors must not panic. No public backend,
plugin, logging, process-management, or notification framework is required.

Before implementation can claim parity, qualify:

1. Native descendant containment, root exit, force stop, timeout, and monitor
   failure, including macOS terminal job control and Windows job lifetime.
2. Safe creation/placement, inherited-resource ownership, activation PID setup,
   and PTY/ConPTY composition. Document any dependency API gap instead of adding
   a project-local unsafe escape hatch.
3. Conmon-compatible log records, driver combinations, caps, rotation/reopen,
   write errors, backpressure, and shutdown sync behavior using source-based
   fixtures. Include partial lines and non-UTF-8 pipe payloads.
4. Filtered notification forwarding and systemd attribution, without synthetic
   readiness or watchdog pings.
5. Held-resource release, exit-helper failure/crash recovery, and retained
   outcomes without false cleanup/durability claims.
6. Existing dillo console-loss detection with full input buffers on all hosts.
   The earlier Linux raw-pipe probe does not qualify PTYs, dillo integration,
   macOS, or Windows.

The remaining architecture work is concrete: macOS containment and safe native
spawn/terminal/resource composition, including the inherited control channel.
These are unresolved requirements, not exclusions accepted by simplification.
No native madrillo tests have run because there is no implementation yet.

[cli]: https://github.com/containers/conmon/blob/46837cd4800889521b6eccd4cce75a69249b1702/src/cli.c
[main]: https://github.com/containers/conmon/blob/46837cd4800889521b6eccd4cce75a69249b1702/src/conmon.c
[runtime]: https://github.com/containers/conmon/blob/46837cd4800889521b6eccd4cce75a69249b1702/src/runtime_args.c
[sockets]: https://github.com/containers/conmon/blob/46837cd4800889521b6eccd4cce75a69249b1702/src/conn_sock.c
[control]: https://github.com/containers/conmon/blob/46837cd4800889521b6eccd4cce75a69249b1702/src/ctrl.c
[logging]: https://github.com/containers/conmon/blob/46837cd4800889521b6eccd4cce75a69249b1702/src/ctr_logging.c
[exit]: https://github.com/containers/conmon/blob/46837cd4800889521b6eccd4cce75a69249b1702/src/ctr_exit.c
[cgroup]: https://github.com/containers/conmon/blob/46837cd4800889521b6eccd4cce75a69249b1702/src/cgroup.c
[oom]: https://github.com/containers/conmon/blob/46837cd4800889521b6eccd4cce75a69249b1702/src/oom.c
[fds]: https://github.com/containers/conmon/blob/46837cd4800889521b6eccd4cce75a69249b1702/src/close_fds.c
[podman-placement]: https://github.com/containers/podman/blob/8d37a39bb1cc07a1425996a8adf24be9213d2f4c/libpod/oci_conmon_linux.go#L164-L181
[podman-runtime]: https://github.com/containers/podman/blob/8d37a39bb1cc07a1425996a8adf24be9213d2f4c/libpod/oci_conmon_common.go#L1100-L1108
[subreaper]: https://man7.org/linux/man-pages/man2/PR_SET_CHILD_SUBREAPER.2const.html
[pid-namespaces]: https://man7.org/linux/man-pages/man7/pid_namespaces.7.html
[cgroup-v2]: https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v2.html
[windows-jobs]: https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects
[windows-assignment]: https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-assignprocesstojobobject
[apple-pgroups]: https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/setpgid.2.html
[conpty-create]: https://learn.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session
[conpty]: https://learn.microsoft.com/en-us/windows/console/pseudoconsoles
[conpty-close]: https://learn.microsoft.com/en-us/windows/console/closepseudoconsole
[portable-pty]: https://docs.rs/portable-pty/0.9.0/portable_pty/
[process-wrap]: https://docs.rs/process-wrap/10.0.1/process_wrap/
[sd-notify]: https://www.freedesktop.org/software/systemd/man/latest/sd_notify.html
