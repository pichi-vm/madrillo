# Madrillo implementation plan

Status: three sequential plan reviews completed, 2026-11-05. See
[scope review](docs/reviews/01-scope.md), [native review](docs/reviews/02-native-lifetime.md),
and [acceptance review](docs/reviews/03-acceptance.md). Ready for implementation
with the stated qualification gates. No conformance completion is claimed.

## Objective and non-negotiable boundaries

Build a small persistent native Rust monitor for arbitrary host workloads,
matching conmon's generic supervision functions. A workload is a launched
leader plus descendants, not just one Child handle. Support native Linux,
macOS, and Windows MSVC. Pichi is the first caller and dillo the first workload;
madrillo contains no VM, guest-execution, image-management, or pichi policy.

Use one package with a bare types-only library and a complete executable.
The library contains shared protocol/result types and serialization derives
only. Default features are empty; executable dependencies require `cli`.
Every owned Rust target uses `#![forbid(unsafe_code)]`. No local unsafe shim,
FFI helper executable, C monitor, or subprocess shell implementation substitutes
for native Rust. Sound dependency APIs may have internal unsafe code.
Operational failures use errors, never production panic/unwrap/expect paths.

The source baseline is C conmon `46837cd4800889521b6eccd4cce75a69249b1702`,
with Podman `8d37a39bb1cc07a1425996a8adf24be9213d2f4c` for caller ownership.
[The source audit](docs/reference/CONMON-COVERAGE.md) records the evidence.
Copy behavior, not bugs or OCI-only invocation conventions. Any deliberate
difference is documented against an acceptance criterion; omissions do not
become acceptable merely because an implementation is difficult.

## Required conformance

Each row requires executable evidence and an explicit platform result. Tests
must use real fixture processes and native facilities. An unsupported required
behavior, ignored test, absent runner, or unimplemented path is not a pass.

| ID | Required behavior | Success criterion |
| --- | --- | --- |
| C01 | Bare library and safe owned code | Default dependency tree has no process/transport/CLI/logging runtime; every target forbids unsafe; production panic audit is clean |
| C02 | Native launch | Preserve native argv/env/cwd including non-ASCII and native non-Unicode values; typed spawn errors; no shell interpolation |
| C03 | Startup and first output | Prepare endpoint/logs/scope before authorized execution; immediate output/exit retained; gated pre-Start EOF launches nothing |
| C04 | Independent lifetime | In captured pipe/terminal modes, workload survives foreground/detached client exit, crash, and terminal disconnect; monitor independent of caller session/job; passthrough documents its external I/O owner |
| C05 | Reconnection | Initial connection and later endpoint attach work; no replay race or unbounded historical backlog |
| C06 | Input ownership | One writer, bounded observers, release on disconnect, explicit pipe EOF; retained writer survives zero clients; Ctrl-D remains bytes |
| C07 | Workload descendants | Force stop, leader exit, and timeout clean the supported workload scope; fork races and grandchildren are tested on every host. Linux/Windows cover changed groups and daemonization. macOS covers descendants remaining in the managed workload process group; escape tests demonstrate the documented boundary |
| C08 | Reaping and result | Preserve leader native exit/signal/full Windows code and exit time; reap owned/adopted children; no fake success or stale-PID kill |
| C09 | Terminal mode | Native PTY/ConPTY, controlling terminal/session, initial size, resize, interactive shell/job control, detach/reattach, drain/teardown |
| C10 | Extra resources | Safe adoption, exact inheritance, capture/pass/hold roles, no leaked writer copies, no argument rewriting or child-API proxy |
| C11 | CRI file logs | Conmon-compatible timestamp/stream/F/P records; partial lines, binary pipe bytes, terminal merged output, extra source separation |
| C12 | Log destinations | File, journald where applicable, file+journal, none, passthrough; actual destination behavior and metadata match |
| C13 | Log controls | Size/global cumulative caps, truncation, rotation/backup count, external reopen, tags/labels, partial metadata, path protection |
| C14 | Pressure and failures | Bounded queues/client count; sink backpressure and write failures explicit; stalled viewer isolated; control/deadline remains responsive |
| C15 | Durability | Default log synchronization and explicit opt-out; atomically retained result; sync/storage failures visible; crash claims precisely limited |
| C16 | Notification relay | Allowed sd_notify messages only, real service attribution, no invented readiness/heartbeat or arbitrary descriptor-store proxy |
| C17 | Socket activation | Correct inherited descriptors, LISTEN_FDS/LISTEN_FDNAMES semantics and actual child LISTEN_PID; unsupported host integration explicit |
| C18 | Runtime timeout | Monotonic deadline in monitor after native launch; works detached; kills workload scope and records timeout separately |
| C19 | Placement | Native membership established before child code/forks; monitor outside workload limits; external manager gate when needed |
| C20 | OOM evidence | Linux cgroup v1/v2 events/final checks and evidence retention; SIGKILL never fabricated as OOM; other platforms report actual available evidence |
| C21 | Monitor OOM protection | Attempt permitted protection; failure visible; protection not inherited by workload or exit helper |
| C22 | Held resources and cleanup | Monitor-only reservations retained then released before final notification; endpoints cleaned; retained state remains readable |
| C23 | Exit command | Native command/args/delay, no shell, no resource leaks; caller policy independent of original CLI; failure does not replace workload exit |
| C24 | Monitor loss | Dillo cooperative cleanup via existing console loss, including full buffers; helpers accounted for; no immediate OS kill preempts cleanup |
| C25 | Protocol and diagnostics | Fixed version, checked lengths/allocations, bounded malformed input; private native endpoint/ACL; separate diagnostics/log levels/native syslog where applicable; CLI help/version/API identity |
| C26 | Pichi integration | Shared run/build launch, run/attach/logs/wait/stop, IDs/discovery/prune protection, terminal restore/detach/resize semantics |
| C27 | Locked build channel | Original inherited named-port pipes, existing command-versioned CBOR request/response, guest-first ordering, virtiofs repo/cache/output remain unchanged |
| C28 | Crash reconciliation | Missing results remain unknown; engine cleanup is idempotent; monitor death cannot claim a callback ran |
| C29 | Native distribution | Native Linux/macOS/Windows MSVC build and execution; reproducible locked dependencies, license inventory, package contains full binary and bare lib |
| C30 | Monitor termination events | Conmon-equivalent Unix signal forwarding without losing reaping/log finalization; native Windows cancellation semantics documented/tested; raw terminal bytes remain separate |

OCI bundle/runtime create/exec/restore construction, guest commands, engine
image management, and engine restart policy are excluded by ownership. An exit
command can invoke engine policy. Platform-specific journald/systemd and Linux
OOM facilities require truthful unavailable results on other hosts, not emulation.
Core launch, pipes, attachment, descendants, timeout, retained results, and
terminal support are required on every native host.

For C27, Pichi's current `BUILD.md` and `pichi-launch` types define the wire
contract: guest request `["build", 1, null]`, then host response `[true, Input]`
or `[false, "error message"]`. The original plan's `v0` label was stale; it is
not an instruction to change or restore a previous guest protocol. Madrillo
passes these resources unchanged and does not interpret their contents.

## Architecture to implement

Keep private executable modules for CLI, framing, supervisor state, native
process/scope/terminal operations, connections, logging, notifications, and
retained state. Prefer small concrete modules over trait/plugin frameworks.
One supervisor owns the lifecycle; bounded workers isolate blocking I/O.
Use a fixed bounded worker set, not one thread per output chunk or unchecked
client. A blocked write/fsync must not prevent force/timeout handling. A sync
deadline can report failure, never claim durable success; do not promise both
unlimited sink progress and bounded resources. Inject these failures in C14/C15.

Use an inherited connected initial channel separate from native stdio. This
supports initial attachment without an output backlog and allows true native
stdio passthrough. Later clients use a Unix socket or Windows named pipe.
Adoption of raw inherited resources must be qualified through safe dependency
APIs before the transport contract is frozen. No number alone transfers a handle.
The initial control endpoint and monitor-owned console writer must never leak
into workload descendants or exit helpers. Duplicating an inherited descriptor
does not close its original; qualification must verify both ownership and exact
close behavior. Terminal and process-scope crates must compose in one actual
spawn, not in separate demonstrations with incompatible launch APIs.

Launch phases are prepare, optional manager placement gate, native scope
assignment, spawn/authorize, supervise, workload cleanup, output drain/log sync,
resource release/result publication, final notification, exit command.
The gate is ordering, not proof of native child membership. Native suspended or
atomic creation may be needed; no workload code executes before containment.
Disconnect before gated Start cancels; disconnect after authorization detaches.

Linux establishes subreaping before launch. After leader termination it stops
and reaps adopted descendants through pidfds, including descendants that changed
groups/sessions, until `waitid` with `__WALL` confirms `ECHILD`. A cgroup supplied
by the caller adds pre-exec placement, resource accounting and a final population
check. V2 adds atomic subtree termination; v1 uses subreaper cleanup. V1 OOM
observation registers an eventfd before launch and checks the final kill counter;
v2 watches `memory.events` with inotify. Observed events create a retained `oom`
marker during execution; final observation never infers OOM from an exit signal.
Windows needs a Job Object with early
membership. The user approved the native conmon process-group contract for
macOS on 2026-11-05: Madrillo creates a workload process group and stops its
members, including ordinary children and grandchildren. Workloads must keep
helpers in that group. Dillo enforces this contract; if it deliberately starts
a helper in another group or session, Dillo owns that helper's shutdown.
Arbitrary escaped-descendant containment is outside the macOS guarantee.
Interactive shells can create separate job-control groups, which have the same
documented limitation. C07 must test both same-group cleanup and this boundary;
it must not claim that a process group contains arbitrary daemonizing programs.

Use optional PTY/ConPTY for host programs. Dillo retains pipe console transport,
not a nested host terminal. Runtime deadline forces scope termination; normal
pichi graceful shutdown remains a direct dillo operation. Monitor-loss cleanup
is dillo's cooperative contract and is distinct from forced termination.
Windows kill-on-last-job-handle-close cannot be enabled with only a monitor-owned
handle if it would kill dillo before console-loss cleanup. Specify handle ownership
and root-crash behavior before selecting a dependency's default drop policy.
Neither a process group nor a fork/exit event stream proves macOS containment
across changes of group or session; that is outside the approved contract.

The primary native exit result does not change if an exit command later fails.
Publish workload cleanup outcome separately from caller-owned external cleanup;
callbacks are not exactly-once across monitor/host crashes. Later pichi
reconciliation is required. Missing evidence remains unknown.

Implement existing CRI file logging and optional native journal integration,
not a new transcript format or library logger. Keep history in configured
destinations. File sync defaults on. Extra captured streams retain identity
without inventing CRI stream tokens. Standard notification relay understands
only its documented allowlist; dillo/build API traffic stays direct.

## Ordered work and milestone exits

1. **Repository and plan:** initialize the standalone repository, include the
   source audit, review this plan three times sequentially, record findings and
   actual fixes. Exit: complete requirement mapping, not a conformance claim.
2. **Native feasibility:** qualify safe handle adoption/forwarding, scoped spawn,
   descendant accounting, terminal composition, activation PID setup, native
   endpoint ACLs, and monitor-loss detection. Add focused real fixtures. Qualify
   the approved macOS process-group cleanup contract before claiming the platform
   implemented. Record dependency
   versions/API constraints and retain any useful experiments as tests.
3. **Package and contracts:** implement minimal shared types, CLI-gated runtime,
   bounded framing, typed errors, retained record schema, and conformance harness.
   Exit: default lib checks and protocol/error tests pass; missing features fail
   explicitly in the coverage manifest rather than passing as skipped tests.
4. **Core monitor:** implement native launch/capture/attach/input/status, startup
   ordering, independent lifetime, scope cleanup, force/timeout, and retained
   state. Exit: real foreground/detached/fast-exit/descendant fixtures pass.
5. **Logging and reliability:** implement all destinations/controls, safe path
   handling, pressure isolation, partial/binary records, reopen, rotation, sync,
   and injected failures. Exit: source-based conmon golden/differential tests.
6. **Terminals and platform services:** implement PTY/ConPTY, resize/job control,
   signal handling, standard notify/activation, placement, OOM, held resources,
   and exit command. Exit: native platform fixtures and Linux service tests.
7. **Pichi/dillo integration:** use isolated branches/worktrees to preserve
   unrelated changes. Integrate shared run/build path and user operations;
   implement full-buffer monitor-loss cleanup and helper coordination. Exit:
   generic monitor tests plus existing platform VM/build checks; no new guest API.
8. **Release conformance:** finish docs/examples/licenses/package checks, audit
   all C01-C30 evidence, run the complete suite and native matrix repeatedly.
   Fix every finding and reset the streak on any change or failure. Exit only
   after three consecutive clean full conformance passes on the final sources.

Work may proceed on independent components while a native API is being
qualified, but no required capability is quietly removed to make a milestone
pass. Tests are behavior-based rather than mirrors of implementation details.

## Three plan reviews

Perform and record separate reviews before creating the implementation goal:

1. Scope against conmon sources and all user requirements; fix missing functions.
2. Native feasibility, unsafe boundary, lifetime/race/error semantics; fix hidden
   assumptions and name unresolved blockers.
3. Testability, implementation order, success criteria, and simplicity; ensure
   passing tests cannot hide absent functionality or change the agreed scope.

Review documents record findings, changes, and remaining risks. Three written
approvals without independent checks do not satisfy this instruction.

## Conformance execution and stopping rule

Maintain a machine-readable requirement/evidence manifest and an executable
runner. Each full pass includes format, warning-clean checks/clippy, minimal-lib
dependency/public-surface audit, unit/integration/failure tests, conmon reference
fixtures, native Linux/macOS/Windows execution, and pichi/dillo integration.
Record source revision/digest, OS/toolchain, test commands, status, and artifacts.
Pin or build the reference conmon used by differential tests; the installed
host version is not automatically the reference revision. Normalize timestamps,
PIDs, and nondeterministic read chunk boundaries when comparing behavior, but
never normalize away lost bytes, stream identity, incorrect status, or ordering.

The evidence manifest begins entirely pending. Each entry names its requirement,
native hosts, test IDs, and report artifacts. A self-declared implemented flag
cannot make a report pass: the runner must validate that required test evidence
exists, completed successfully, and corresponds to the current source digest.
Keep infrastructure/build smoke checks separate from full conformance reports.

Failure injection includes full disk/write/sync error, broken endpoints,
malformed/truncated/oversized protocol, rapid spawn/exit, descendants changing
groups, blocked input/output, and callback failure. Resource-limit/OOM tests use
isolated bounded fixture scopes and must not induce global host memory pressure.
Repeated conformance rounds include independent fresh instances and race/stress
iterations, not just rerunning one happy-path unit test three times.

Three consecutive clean passes must use unchanged final implementation sources.
Any failed, skipped-required, unrun, or unimplemented case makes that full pass
unclean and resets the streak. A Linux-only pass or cross-compilation is useful
evidence, but not a full native conformance pass. Platform-specific facilities
may be not-applicable only where the requirement explicitly says so; core
capabilities may not be reclassified that way. Final reporting includes all
three reports and any deliberate compatibility differences.

Current host is Linux. macOS/Windows execution will need native runners; prepare
CI and use available authorized repository infrastructure. Do not label an absent
runner successful. Repository creation and implementation are authorized; keep
unrelated workspace changes intact. No goal is complete until this stopping rule
is met or the user changes the objective.
