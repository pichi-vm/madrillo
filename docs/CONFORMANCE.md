# Full conformance runner

Qualification CI is useful evidence and does not count toward the three-pass
stopping rule. `tools/conformance.py` is the full-conformance gate. The current
manifest deliberately remains pending, so this command records failure before
executing any suites:

```
python3 tools/conformance.py run --round round-1 --report conformance-runs/round-1/linux.json
```

Each reviewed C01–C30 requirement must have `status: ready`, explicit native
hosts, and executable test mappings. `ready` means coverage is ready to run;
it does not mean the requirement passed. Test cases require positive output
markers as well as exit zero, so a filter selecting zero tests is not success.
The current case mappings are partial and must be completed before promotion.

Run the same round on Linux, macOS, and Windows MSVC with unchanged, clean
sources. Collect each JSON report together with its sibling log artifacts.
The aggregator checks every required native case, source revision/tree, round
identity, actual exit status, output markers and artifact hashes:

```
python3 tools/conformance.py aggregate --round round-1 --ledger conformance-runs/ledger.json conformance-runs/round-1/linux.json conformance-runs/round-1/macos.json conformance-runs/round-1/windows.json
```

Both the native runner and aggregator require compiler-target evidence for the
reported operating system. Windows must use MSVC; a MinGW/GNU report cannot
count even when its tests returned zero. Missing compiler evidence also fails.

Aggregate failed and missing-native rounds too; they reset the streak. Reusing a
round or native invocation cannot count again. Changes to source identity start
a new streak. Three fresh successful aggregates on the same source identity are
required. Reports and the ledger remain outside tracked implementation sources.
The manifest's initial `clean_streak` is not accepted as evidence by the runner.

Pichi and Dillo are part of the source identity. `CONFORMANCE.json` pins their
repositories and immutable revisions; the native runner and aggregator require
independent checkouts at `.qualification/components/pichi` and
`.qualification/components/dillo`. Every checkout must be clean, including
initialized recursive submodules at their pinned revisions. An empty component
directory cannot fall back to the enclosing Madrillo repository. The reports
include each component's revision and Git tree. Changing either component
starts the streak again, even when the monitor itself has not changed.

Remaining harness work includes full failure-injection and native case mappings,
pinned conmon build/differential artifacts, pichi/dillo integration evidence,
and native CI round orchestration. Until those are present,
the full runner must fail closed while any requirement remains pending.

The manually dispatched `dillo.yml` workflow qualifies the selected immutable
Dillo revision against the monitor built from that workflow's source. It runs
three real VM console-lifetime cases on native KVM, HVF and WHP, including the
explicitly selected monitor-crash case. `tools/check_dillo.py` rejects zero-test
or ignored-only runs and requires guest input, sustained unread-input pressure,
and a cooperative zero exit after monitor death. A separate native pipe test
proves hangup detection with unread bytes and verifies that observation consumes
none of them. The C24 cases run these checks against the pinned Dillo checkout;
the requirement remains pending until its complete native review finishes.
Linux fixtures need 128 free 2 MiB hugepages for their 256 MiB guests. This focused
workflow does not replace the full VM suite.

`tools/check_pichi.py MONITOR PICHI_CHECKOUT` runs the caller unit/CLI cases and
explicit native launch and monitor-crash fixtures. On Windows it invokes the
existing native test host outside the runner's restrictive Job, including actual
caller console-window closure. The command requires positive execution markers
for unknown-state retention, idempotent storage cleanup, image pins and native
crash reconciliation. It is shared by focused CI and the C28 case mappings; VM
and locked build qualification remain separate requirements. Madrillo's native
control tests additionally kill the monitor before publication and between
publication and a delayed exit callback, checking retained evidence in each case.

## Reviewed native launch and reconnection coverage

C02 is ready to execute on all three native hosts. The required positive markers
cover literal shell metacharacters, empty arguments, trailing backslashes,
embedded quotes, native non-Unicode argv/environment, working-directory
selection, and retained spawn failure. Both pipe and terminal modes run the
native context fixture. APFS rejects invalid filename bytes itself; that case
uses a Unicode working directory while still verifying raw argv/environment.
The source uses native process APIs and retains a typed `SpawnFailed` outcome.

C05 is ready to execute on all three native hosts. Required markers cover an
inherited initial connection receiving immediate first output without sending
a handshake, endpoint reattachment after writer disconnect, terminal reconnect,
and a draining viewer receiving a large startup burst. The disconnect fixture
checks that a new attachment receives only newly emitted bytes. Source review
confirms attachment queues contain pending delivery only: no historical replay
buffer or sequence protocol is present.

These mappings add required positive markers to the existing native test suite;
they do not rerun the suite for each requirement. Readiness is a coverage review,
not a conformance pass. Caller terminal/Job loss remains separately tracked by
C04, and Windows terminal descendant cleanup remains pending under C07/C09.

C20 is ready to execute with real cgroup v2 and an isolated cgroup v1 VM on
Linux. Both require pre-launch event registration, bounded OOM allocation,
final counts, live marker retention across monitor death, explicit marker write
errors, and rejection of removal as an OOM event. Native Unix signal and Windows
full-exit-code tests separately require unavailable evidence when no OOM source
exists; an exit code or signal alone cannot satisfy the requirement. The full
Linux runner needs the same QEMU/cpio/kernel-download prerequisites as native
qualification CI; their absence fails the case instead of skipping v1.

C21 is ready to execute with separate permitted and denied Linux OOM-adjustment
cases. Each checks the live monitor, workload, and exit-helper scores in both
pipe and terminal modes. The denied case requires the retained warning; the
permitted case requires protection without that warning. A case cannot accept
whichever branch happens to occur. macOS and Windows have no Linux OOM-score
mechanism; their native launch tests verify operation without that facility.

C23 is ready to execute on all three hosts. The exit helper sees the retained
workload result and receives literal native arguments, including non-Unicode
values. The success case checks the requested delay. Nonzero helper exit and
failure to spawn the helper leave the workload's exit and logs intact. Existing
reservation and capture fixtures require release before the helper runs; Linux
also checks restoration of the inherited OOM score. Source review confirms
connections close before the helper starts and workload activation/notification
environment variables are removed. The helper cannot substitute its result for
the already-published workload result.

C11/C12 are ready to execute with native CRI, partial/binary pipe, merged
terminal, extra-stream, disabled-log and passthrough cases. Linux additionally
builds the pinned conmon source afresh and compares real file and journal
records, including combined destinations, metadata and partial-message options.
The large-journal case must execute its normally ignored native memfd fixture.
macOS and Windows explicitly reject requested journald integration before the
workload runs. Log size/rotation and storage-failure coverage remain separate
under C13-C15; these entries do not claim those requirements are complete.
The terminal fixture writes Unicode text because
[Rust's Windows console writer](https://doc.rust-lang.org/std/io/struct.Stdout.html#note-windows-portability-considerations)
rejects invalid UTF-8. The pipe fixture continues to require arbitrary binary
bytes, including NUL and `0xff`, without conversion.

C16/C17 are ready to execute against Linux's actual systemd notification and
socket-activation facilities. The fixtures check notification filtering, service
attribution, oversized datagrams, pressure during force/timeout, and two named
sockets whose metadata identifies the actual workload PID. macOS and Windows
must reject explicitly requested systemd facilities before launching a workload.

C29 now also requires the native RPM build/payload check on Linux. That case
needs Fedora's cargo-rpm-macros, RPM build tools, native Rust/C compilers and
Python. It generates the source RPM in temporary storage, rebuilds it offline,
executes the packaged monitor, and verifies the executable and license payload.
Missing build tools fail the case. A full round also needs a fresh
`.qualification/reference` directory: the conmon builder rejects reuse of an
existing oracle source tree.

C03/C06 are ready to execute on all native hosts. Startup cases require the
prepared endpoint/logs, immediate output and exit, cancellation on pre-Start
disconnect, and execution only after authorization; Linux also runs real
pre-exec cgroup placement fixtures. Input cases cover the single writer,
literal Ctrl-D/binary pipe bytes, disconnect and InputEnd handoff, retained
output attachment after handoff, explicit administrative EOF, and draining
accepted bytes before EOF. The observer-capacity fixture fills the endpoint
until an excess client is rejected, then uses an existing connection to start
and terminate the workload. A rejected attachment cannot stand in for a dead
or unresponsive monitor.

C22 is ready to execute with real held-file reservations, capture cancellation,
and retained completion. The initial-channel fixture reads and compares
`result.json` as soon as Completed arrives, and checks Unix endpoint removal.
The Linux notification fixture also requires relay socket removal. Source review
places held-resource release and listener closure before publication and final
notification on every host; live connections remain only for bounded delivery.

C30 is ready to execute with real Unix SIGINT/SIGTERM/SIGHUP/SIGQUIT delivery,
workload handlers, retained logs and native signal exit status. Separate native
shell fixtures exercise Ctrl-C through PTY/ConPTY after detach/reattach. Windows
forced cancellation uses the authenticated Terminate request; killing the
detached monitor is monitor loss. The force/descendant fixture exercises that
request without hiding the separately pending ConPTY Job-empty observation.

C08 is ready to execute. It requires a Linux fault fixture that delays the actual native
termination syscall while the workload continues recording wall-clock samples.
The retained exit timestamp must follow the last live sample. It represents
Madrillo's observation of terminal status, not the instant a stop was requested
or an exact kernel exit timestamp. A previously observed exit time survives
later cleanup errors; an unobserved exit has timestamp zero. Native core cases
also check that known outcomes carry a timestamp by completion. Required native
cases preserve full Windows exit bits, distinguish Unix signals from numeric
exit codes and OOM, and reap adopted Linux orphans while the leader still runs.
Source review keeps Unix leaders waitable until force delivery, validates
adopted Linux children through pidfds before signaling, and uses owned process
handles on Windows. These identity guarantees prevent stale numeric-PID kills.
A second fault delays descendant-cleanup completion beyond the monitor's reap
deadline. The leader's observed status and timestamp must survive while cleanup
remains explicitly incomplete; cleanup timeout must not turn a known exit into
Unknown.

C14/C15 include Linux write/fsync completion stalls, synchronization errors and
opt-out, an isolated full filesystem, and failed result publication. During a
stall, the test requires retained completion within ten seconds while strace
holds the log worker for twenty seconds. It then lets the tracer unwind and
checks the monitor's exit. This measures bounded supervision and publication;
the tracer can postpone OS thread exit, so its lifetime is not the shutdown
deadline. The result must explicitly report truncated/incomplete logging.
The ordinary blocked diagnostic-pipe test runs natively on every host and
requires monitor exit as well as retained timeout/flush errors. C14 is ready to
execute with those cases, stalled-viewer isolation, explicit client-capacity
rejection and blocked-child-input force/timeout coverage. Source review bounds
capture/live queues to sixteen 8 KiB chunks, input to 64 KiB pending bytes,
clients to 32 and each client's pending delivery to 1 MiB. Blocking log work has
one worker; capture workers and Windows final-delivery workers have fixed bounds.
This is backpressure with explicit failures, not a promise to retain unlimited
output from a stalled sink.

C15 is ready to execute with native atomic-publication/collision checks and
Linux syscall/storage fault injection. Source review confirms default file-log
synchronization, independent result-file synchronization, no-overwrite rename,
and Unix directory synchronization before final acknowledgement or callback.
The log-sync opt-out cannot disable result synchronization. Claims are limited
to the operating system/filesystem guarantees documented in README.md.

The storage suite also injects EIO into the exact state directory's post-rename
fsync. The monitor must fail visibly without sending Completed or running the
exit command, while preserving the already published leader status. This differs
from a full-filesystem failure before publication, which must leave no result.
Neither record presence nor missing acknowledgement proves an exit hook ran.
An additional native fixture places an existing file or nonempty directory at
the result path before completion. Publication must fail without overwriting it,
leaving a temporary result behind, acknowledging completion or invoking a hook.

C25 is ready to execute. Positive markers cover version, invalid lengths and
encoding, unknown messages, truncated frames, bounded outgoing encoding, and
CLI help/version/API identity without creating state. Real clients leave partial
headers and payloads open while another client terminates the workload; invalid
versions, byte values, UTF-8 and deeply nested JSON must not stop supervision.
The Unix peer fixture requires another account to connect through a public
socket and be rejected before its request runs. Windows fixtures inspect the
actual endpoint DACL and private state inheritance. Native Linux syslog/macOS
unified-logging fixtures require delivery and workload/diagnostic separation;
every host checks diagnostic levels, aliases and blocked stderr. Frame storage,
client counts and per-turn servicing are explicitly bounded in the executable.

The input-pressure fixture sends enough data to fill a non-reading workload's
native pipe and bounded pending queue, requires ResourceLimit, then requests EOF.
Both a separate client's force request and the detached runtime deadline must
still finish with complete pipe-workload cleanup on every native host.

C13 is ready to execute. Its Windows path checks inspect the held directory handle's owner and DACL
before opening or creating a log. Only the invoking user, SYSTEM and
Administrators may receive modification rights; other accounts may receive
read/execute rights. The mask follows Microsoft's
[file-access mappings](https://learn.microsoft.com/en-us/windows/win32/fileio/file-security-and-access-rights)
and [generic-access bits](https://learn.microsoft.com/en-us/windows/win32/secauthz/generic-access-rights).
Native tests require public read access to work and reject write, delete-child,
ownership-change and DACL-change grants, with and without an allowlist. Existing
files must remain unchanged and rejected new files must not be created.
The pinned-conmon differential also compares every active/backup log in eight
file-control cases: truncation, one/three backups, oversized whole records,
cumulative caps with and without rotation, existing-file append and an already
exhausted cap. Timestamps are validated then normalized; payload bytes, stream
identity, partial/full markers, record order and backup names must match.
