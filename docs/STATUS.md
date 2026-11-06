# Implementation status

Goal active. Three sequential plan reviews are complete. Full conformance:
**0 of 3 consecutive clean passes**. All C01-C30 entries remain pending.

Implemented so far:

- Standalone repository, pinned Rust toolchain and dependency lockfile.
- Types-only default library; executable and qualification dependencies are gated.
- Protocol/result type draft and explicit unsafe prohibition on owned targets.
- Bounded framing with six passing tests for malformed/truncated/oversized
  input, fixed version, binary payloads, and bounded encoding without partial
  oversized transmission.
- Captured pipe launch, native argv, retained stdin writer, monotonic timeout,
  bounded capture/log workers, binary CRI logs, file caps/rotation and fsync,
  private state directories and atomic result publication. Live attachment and
  Unix terminal operation are implemented as described below.
- Real process tests exercise immediate output/exit, timeout with continuous
  output, retained stdin, native argv, spawn failure and a descendant remaining
  in its scope. These do not prove escaped-descendant containment.
- Native build/qualification CI, explicitly separate from full conformance.

The repository is https://github.com/pichi-vm/madrillo (now public). Native foundation CI
passed on Linux, macOS, and Windows MSVC at `bd716e3313688493d1d0f0aa40c7e5921f58d326`:
[run 42095614947](https://github.com/pichi-vm/madrillo/actions/runs/42095614947).
This does not increment the full conformance streak.

Native qualification findings:

- `command-fds` 0.3.3's receiving initialization requires an unsafe call and
  `/proc/self/fd`; that initializer is unsuitable for madrillo.
- `filedescriptor` 0.8.3 offers safe duplication from a borrowed raw descriptor.
  The Unix inherited-pipe experiment tests duplication followed by closing the
  original through nix's safe close API. This is not yet proof of arbitrary
  handle adoption on Windows, full validation, or complete inheritance control.
- `portable-pty` 0.9.0 establishes a controlling terminal through its safe API,
  but its Unix spawn closes extra descriptors and does not expose the underlying
  Command for scope/inheritance composition. It is not selected as a complete
  solution merely because it compiles.
- An alternative Unix composition passed on Linux: allocate a PTY with safe
  nix APIs, spawn a fresh execution of the same Rust binary, establish its
  session/controlling terminal with safe nix/rustix calls, then use safe
  CommandExt::exec to replace it with the workload. This preserved an extra
  inherited pipe, actual PID in LISTEN_PID, terminal size, and native exit code.
  It needs no persistent extra process and no unsafe pre_exec callback. macOS
  execution also passed in [run 42096553055](https://github.com/pichi-vm/madrillo/actions/runs/42096553055);
  cgroup placement is not yet qualified.
- `process-wrap` 10.0.1's Windows JobObject uses suspended creation/assignment
  and requests `make_job_object(..., false)` without mandatory kill-on-close.
  This is promising for dillo cleanup, but combined PTY/inheritance behavior
  and actual native Windows execution remain unqualified.

The new live endpoint supports bounded clients, a single input owner, detach
and reconnect, explicit pipe EOF, force requests and completion delivery. Tests
cover malformed clients, a stalled viewer, owner release and draining accepted
input before EOF. No historical output is buffered for replay. The Unix
inherited initial connection now retains first output without a client handshake;
the optional gate cancels on pre-Start disconnect and detaches after Start.
Both behaviors have executable tests. Windows inherited control has now been
implemented and is awaiting native qualification, as described below.

The executable uses Unix process groups as an implementation step and explicitly
records incomplete cleanup guarantees in its result when no stronger scope is
provided. Caller-owned Linux cgroup v2 containment is now implemented. Windows uses process-wrap's suspended Job Object assignment;
the new end-to-end path is awaiting native CI. Neither implies a full C07 pass.
Core CI at `9eaeef9` passed on Linux/macOS and the Linux conmon comparison
passed in [run 42099759289](https://github.com/pichi-vm/madrillo/actions/runs/42099759289).
Windows stopped at a cfg-specific unused-mut warning before tests; that warning
is fixed. Windows runtime success is not inferred from cross-compilation.

The next run at `de04499` passed Linux/macOS attachment tests but exposed a real
Windows stdin-handle permission mismatch. Input now creates its pipe before
launch instead of attempting to rewrap std's write-only child handle. Explicit
bounded close/final-delivery workers replace implicit unbounded pipe-drop work.
Native Windows retesting is pending.

Further Windows execution at `0c372c5` exposed a library behavior: a nonblocking
empty named pipe was reported as EOF. The transport now uses a safe native
PeekNamedPipe wrapper to distinguish an empty live pipe from disconnection,
while leaving overlapped reads/writes with Interprocess. Child stdin now uses
a synchronous client handle, as arbitrary child programs expect.

Unix terminal operation is implemented and tested on Linux: controlling TTY,
size/resize, merged capture, persistent detach/reattach, and native exit status.
The fresh same-binary launcher reports setup/exec errors through a close-on-exec
error pipe. This is internal exec error reporting, not a client handshake.
ConPTY integration is now awaiting native verification, as described below. Unix termination-signal forwarding has a
real process test. Reaping and retained-state fsync waits are now bounded.

The pinned conmon source has been built locally as a test oracle. The same
binary fixture produced matching CRI binary payloads and exit 37. The committed
CI harness independently rebuilds the pinned archive and verifies its digest
before differential tests; twelve comparisons passed in that Linux CI run.

Next: native CI of core launch, live attachment/input/control, inherited initial
connection, bounded finalization, native scopes and terminals. macOS arbitrary
descendant containment remains unresolved; a scope clarification is pending.

Linux cgroup qualification now passes locally: membership before first workload
code, process-tree cleanup after a descendant calls setsid, both pipe and PTY
composition, bounded 32MiB cgroup OOM evidence, and rejection of fake control
files on ordinary filesystems. The monitor uses subreaping throughout the leader's
lifetime and waits for group emptiness plus adopted-child reaping at cleanup.
Only caller-supplied, initially empty cgroup v2 scopes are supported so far; v1
OOM evidence and monitor OOM protection remain pending. The caller owns group
creation/limits/deletion and must keep the monitor outside the workload group.
The fresh Unix launcher is shared by pipe and terminal setup, never persists
after exec, and reports native launch errors on a close-on-exec pipe.

Native run `d08f5d9` passed Unix terminal checks on macOS/Linux and fixed three
Windows control tests. Large control frames still exceeded their test deadline;
larger explicit pipe buffers alone did not fix it at `a42c800`. Windows writes
are now split into bounded 8KiB native operations, with exact progress included
in a failed test diagnostic. Native verification of that change remains pending.

A safe ConPTY implementation now uses conpty-oxide 0.1.2, whose windows-spawn
0.1.0 dependency composes pseudoconsole creation with Job assignment. The OS
ConPTY backend is selected explicitly, without adjacent DLL discovery. Terminal
input runs through one bounded worker and output uses the existing capture path.
Tests cover size/resize, detach/reattach, native exit and descendant cleanup on
all native CI targets. Windows execution is pending. This is not yet full C09:
extra inherited handles are unsupported by this dependency, and it does not
expose a job-empty observation. Windows terminal results therefore truthfully
record incomplete cleanup evidence even after requesting tree termination.
Its terminal Job uses kill-on-close; the separate pipe path used by dillo retains
its no-kill-on-close policy so it can perform console-loss cleanup.

Completion now separates monitor errors from log errors, and the listener is
released before result publication/final notification. Launch failures also
send the retained completion to an existing initial client.

At `3c172e2`, Linux/macOS qualification passed including the real Linux cgroup
suite; all four Windows attachment tests passed. Windows core testing then
exposed a process-wrap completion-port race: its outer try_wait consumed the
Job-empty event that its later wait needed. Madrillo now polls the inner leader
and reserves the Job event for final cleanup. An already-observed native leader
result is retained separately even if cleanup/reaping times out on any host.

Native qualification passed on all three hosts at `7556611`:
[run 42106188595](https://github.com/pichi-vm/madrillo/actions/runs/42106188595).
This includes actual Windows ConPTY size/resize, detach/reattach, native status
and descendant-termination fixtures, plus the pipe leader/Job wait fix. It is
still a partial qualification run, not a full conformance pass.

File reopen now has a bounded asynchronous control request and completion
response. Rotation creates the replacement first, preserves the active log on
failure, syncs retired files unless opted out, and syncs the held directory at
finalization on Unix. New tests check record continuity across explicit reopen,
failed-rotation preservation, and Windows hard-link rejection. Native verification
of these latest logging changes is pending.

Native logging qualification passed at `396f93b`:
[run 42106841597](https://github.com/pichi-vm/madrillo/actions/runs/42106841597).
The cumulative file cap now includes pre-existing active-file bytes, matching
conmon's initialization (with a focused regression test).

Journald is implemented using systemd's documented native datagram/memfd
protocol, without a C-library dependency or lossy UTF-8 conversion. It supports
conmon-compatible identity/name/tag/label fields, priorities and priority
prefixes, bounded partial lines and the partial-marker opt-out. File+journal,
none/off/null and native passthrough are selectable. Metadata and record sizes
are bounded. Real local journald differential tests against the pinned conmon
binary passed for binary data, complete/partial lines, priority prefixes split
across reads, labels/tags and combined file output. The same test is in Linux CI;
other hosts reject journald explicitly. The memfd fallback is implemented but
has not yet been independently exercised. Full logging failure injection and
path-allowlist coverage remain pending.

Windows initial-channel adoption now duplicates and validates the named pipe,
checks that it has a peer, closes the transferred original, and retains only
non-inheritable monitor ownership. Safe windows-spawn argument-handle lowering
is used by the native test launcher, so the numeric argument denotes an actual
inherited child handle rather than a copied parent number. The same no-handshake,
pre-Start cancellation and terminal startup tests now execute on Windows too.
Invalid initial resources are rejected before state creation. Native verification
is pending; exact arbitrary-handle pass/hold/capture support remains unfinished.

All native CI jobs passed at `9fc5e58` (including real journal differential
coverage) and at `cc5506f` (including Windows inherited startup):
[run 42108035781](https://github.com/pichi-vm/madrillo/actions/runs/42108035781),
[run 42108356313](https://github.com/pichi-vm/madrillo/actions/runs/42108356313).
The Windows no-handshake and gated cancellation/authorization paths are now
executed on Windows MSVC, not inferred from cross-compilation.

The Linux notification relay is implemented with conmon's exact safe-field
allowlist, a private workload socket, bounded pending datagrams, and explicit
failure reporting. It accepts filesystem and abstract systemd destinations.
It sends no synthetic readiness or watchdog traffic and does not rewrite sender
credentials. Local qualification passed both exact datagram filtering and a
real Type=notify service with NotifyAccess=main; the service manager accepted
readiness/status from the actual madrillo MainPID. Native CI verification is
pending. Non-Linux hosts reject the systemd-specific option explicitly.

Native notification qualification passed at `89c05df` on all three hosts:
[run 42109457350](https://github.com/pichi-vm/madrillo/actions/runs/42109457350).
Final notification draining now has a deadline and explicit retained failure
evidence; queued delivery is not silently treated as success.

Linux now attempts conmon's monitor-only OOM adjustment, restoring the inherited
value in the fresh workload launcher before user code. A failed protection
attempt remains a retained monitor warning and does not prevent execution or
contaminate passthrough stderr. Local pipe/terminal tests passed with denied
protection; CI additionally requires successful privileged protection. Restoration
for the exit helper will accompany exit-command implementation. Local full
current tests and Windows cross-clippy passed; full C01-C30 streak remains 0.

Native OOM qualification passed at `1b543c8` on all three hosts:
[run 42110646765](https://github.com/pichi-vm/madrillo/actions/runs/42110646765).
The Linux job verified successful privileged protection and restored workload
policy; local unprivileged denial remains explicitly visible in retained warnings.

Exit commands now run after publication/final delivery with native repeated
arguments and optional delay. Their failure changes the monitor's command outcome
without changing the retained workload result. Linux restores the original OOM
adjustment in exit helpers, and activation/notification environment is removed.
The callback has conmon's explicit-command lifetime; no new callback timeout or
exactly-once promise is introduced. Native-argument, delay, failure and OOM
restoration tests passed locally; native CI verification is pending.

Resource adoption now validates the whole bounded transfer before closing any
original. It rejects duplicate roles and a subtle invalid-number case where an
earlier duplicate could otherwise allocate that number. `--hold` retains only a
non-inheritable copy until cleanup and releases it before publication/notification.
The native reservation fixture uses Unix file locks or Windows exclusive file
sharing and checks release in the exit command. Windows held-file verification
is pending; Windows socket reservations have not been qualified.

Unix `--inherit` preserves exact requested descriptor numbers in pipe and PTY
launches. Internal setup descriptors are assigned beyond requested numbers, and
unlisted descriptors close in both monitor adoption and the fresh launcher. No
child argument is rewritten. Local tests cover descriptor 21 (the former fixed
setup descriptor), an unlisted descriptor and duplication-time invalid claims.
Windows exact extra-handle inheritance still rejects explicitly rather than
claiming a working contract; named extra capture is still pending.

Linux systemd socket activation now adopts the FD 3.. range and names, checks the
PID/count contract, and sets LISTEN_PID to the actual workload in the launcher.
`--replace-listen-pid` permits the explicit manager handoff conmon supports.
Stale LISTEN_PIDFDID is removed when forwarding to a different process. Real
systemd-socket-activate tests passed for two named sockets in pipe and PTY modes.
Local current tests, cgroup placement/escaped-descendant/OOM tests, notification
service tests and Windows cross-clippy passed. Full conformance remains 0/3.

The executable conformance gate is in tools/conformance.py. It requires reviewed
coverage mappings, clean source identity, positive execution markers and native
Linux/macOS/Windows reports with matching log hashes. Aggregation records failures
and resets the streak; repeated invocation/round IDs cannot be counted twice.
Five synthetic negative tests passed for missing/modified/nonexecuted/failed
evidence, changed sources, repeated rounds and pending requirements. The actual
full preflight correctly failed closed: all C01–C30 coverage remains pending.
Full native orchestration, complete case mappings and pichi/dillo source/evidence
are still required. These gate tests are not workload conformance evidence.

Native resource and exit-command qualification passed at `bdfdbfd` on all three
hosts: [run 42111864394](https://github.com/pichi-vm/madrillo/actions/runs/42111864394).
This includes actual Windows held-handle release before the callback, native
callback arguments/failure, and Unix descriptor/PTY/activation composition.

Windows console-input and control pipes now grant access explicitly to the
invoking token's user SID, instead of the owner-rights alias (which may refer
to a group-owned object). The safe win-custody 0.1.2 API supplies the SID; the
existing interprocess API applies the descriptor. A Windows native test queries
the resulting pipe ACL and checks that its sole allowed trustee is that user.
Cross-clippy and bare-library checks passed; native verification is pending.
Atomic private state-directory creation on Windows remains unfinished.

The conformance-gate negative tests passed in all three native jobs at `9a59fab`:
[run 42112570294](https://github.com/pichi-vm/madrillo/actions/runs/42112570294).
These validate rejection behavior, not a clean full-conformance round.

File logs now support repeated --log-allowlist-dir. Resolution stays inside a
held allowed-directory capability; a prefix-similar sibling or parent-symlink
escape cannot gain access. Unix rejects world-writable or unexpectedly owned
log directories. The check also applies to existing logs, deliberately closing
the pinned conmon revision's existing-file bypass of its allowlist check.
Focused tests for existing/new paths, directory boundaries, symlink escape and
world-writable parents passed locally, as did Windows cross-clippy. Full native
qualification is pending; Windows reparse-race fault injection remains pending.

Native launch qualification now includes non-Unicode argv/environment/cwd, empty
arguments, trailing backslashes and embedded quotes through both pipe and host
terminal launch. The fixture writes raw native encodings to a file, avoiding
terminal text normalization as an accidental oracle. Local Unix execution and
Windows cross-clippy passed; macOS/Windows native execution is pending.

Linux storage-fault qualification now exercises actual fsync EIO injection
(scoped by strace to the exact owned log path), the --no-sync-log opt-out, a
64KiB tmpfs filling during a detached flood with a live monitor deadline, and
failed retained-result publication on that full filesystem. Publication failure
must produce neither a final Completed frame nor an exit-command invocation.
The callback test uses an unconditional marker command so a missing result
cannot make an incorrectly invoked callback look uninvoked. Local qualification
passed inside a private user/mount namespace; no host disk was filled. The same
check is added to Linux CI. Other-host storage fault injection remains pending.

Native Windows pipe ACL qualification passed at `f9ba357`, and native log
allowlist qualification passed on all hosts at `b8988ff`:
[run 42113030898](https://github.com/pichi-vm/madrillo/actions/runs/42113030898),
[run 42113359838](https://github.com/pichi-vm/madrillo/actions/runs/42113359838).

At `4a620b8`, Linux and Windows native tests passed, including Linux storage
faults and Windows non-Unicode argv/environment/cwd through ConPTY. macOS rejected
the invalid-byte directory name with EILSEQ before madrillo ran. The native test
now keeps that exact OS rejection path and uses a valid Unicode cwd on that
filesystem; non-Unicode argv/environment remain required and are not skipped.
Other creation errors still fail the test. Native rerun is pending.

A native pipe-liveness experiment now fills input before closing the sole
writer. Local Linux proves POLLHUP without consuming the 8192 queued bytes.
The Windows experiment will report PeekNamedPipe's behavior with unread data;
its observation is not a claim that dillo monitor-loss integration is complete.

The Windows experiment at `b621372` observed PeekNamedPipe returning queued
input after the server closed: 65536 bytes remained readable, and BrokenPipe
appeared only after draining them. Thus peeking alone does not satisfy full-buffer
monitor-loss detection. The experiment is extended to query the pipe's server
process/session metadata without consuming data; native observation is pending.
No new heartbeat/liveness channel or child API proxy has been introduced.

Exit-result qualification now tests Unix SIGKILL/SIGTERM separately from
numeric exit values and OOM evidence, in pipe and PTY modes. Windows adds full
32-bit exit values (including 259/STILL_ACTIVE and high-bit values) in pipe and
ConPTY modes. Local Unix execution and Windows cross-clippy passed; native
Windows verification is pending.

Native qualification at `dd2e18a` passed on Linux, macOS and Windows, including
Unix signal/native-result separation and Windows 259/high-bit exit values:
[run 42116263975](https://github.com/pichi-vm/madrillo/actions/runs/42116263975).
The macOS non-Unicode-context correction also passed. Full conformance streak
remains zero.

At `3ef2951`, macOS and Linux reported POLLHUP with unread input. Windows still
returned the old server PID/session after close while 65536 bytes remained
buffered: [run 42115778798](https://github.com/pichi-vm/madrillo/actions/runs/42115778798).
Thus these Windows metadata queries do not solve full-buffer console loss.
Microsoft documents a native closing state even with unread pipe data; that
lower-level dillo-side mechanism needs qualification. No liveness protocol was
added. An isolated dillo worktree is prepared; no dillo source has changed.

Named extra captures now use `--capture NAME RESOURCE`, own a nonblocking pipe
read end, publish the selected stream name live, and write a separate raw log.
They share the bounded logging queue and existing rotation/sync/path controls,
without introducing CRI tokens or parsing the child's data. Cancellation joins
extra readers before final publication/callback even if the external writer
stays open. Windows rejects overlapped capture handles explicitly and uses safe
winx mode queries (already a transitive dependency) to validate synchronous I/O.
Local binary/PTY capture, exact raw caps/rotation, live Unix selection combined
with unchanged inherited workload arguments, invalid-resource rejection and
open-writer cancellation passed. All local tests and Windows cross-clippy
passed. Native Windows/macOS qualification is pending; C10 remains incomplete
because exact Windows workload inheritance is not implemented.

Named capture at `54b9e24` passed natively on all three hosts:
[run 42118004018](https://github.com/pichi-vm/madrillo/actions/runs/42118004018).

Dillo integration has started in the isolated `feat/madrillo-console-lifetime`
branch at `dd8e8d190e5c31552a1e2aab336418c1d7f1210d`. Its opt-in
--exit-on-stdin-close observes the existing console pipe independently of guest
reads and requests the existing vCPU stop/VM teardown path. Windows queries the
native pipe closing state in dillo's OS code; madrillo remains entirely safe
Rust and gains no liveness channel or dependency on dillo. Linux/macOS use poll;
macOS requires read readiness to be requested before it reports hangup.
All native pipe tests now pass with 65536 unread bytes:
[run 42119605985](https://github.com/pichi-vm/dillo/actions/runs/42119605985).

Dillo's old port-zero RX queue had no stdin producer. A process-wide bounded
reader now supplies it without holding VM references or blocking device-worker
teardown. A local KVM test boots a healthy guest, requires proof of input reaching
that guest, closes the console writer, and observes zero exit without force-kill.
Native WHP/HVF VM execution and actual madrillo-crash integration remain pending.
The broader dillo CI is running; the user is restarting ARM64 Linux runners, so
runner interruptions will be treated as transient and retried. No source change
has been made to the user's existing dillo worktree or its uncommitted test fix.

Windows state creation now validates a held parent's owner and inheritable DACL
before mkdir. Only the invoking user, SYSTEM and Administrators may have access,
and full user access must inherit to directories/files. This uses native ACL
inheritance from the instant of creation; it does not create public state and
repair it afterward. Native tests check the directory, log and retained result,
and require an Everyone-writable parent to fail before state exists. This path
is awaiting Windows execution; it is not yet a C25 conformance claim.

Windows state ACL inheritance at `c7a03d5` passed natively, including private
result/log ACLs and rejection before creation under an Everyone-writable parent:
[run 42120489081](https://github.com/pichi-vm/madrillo/actions/runs/42120489081).
Linux passed in that run; macOS remains queued.

Unix listener admission now checks the kernel-reported effective UID before
processing requests or sending output. A native two-account qualification
deliberately makes the socket public, requires another account to connect and
be rejected, then verifies the original owner can still exchange input/output
and close the workload normally. Inherited initial control remains an explicit
transferred capability. macOS execution of this check is pending.

The dillo VM suite at `dd8e8d1` passed on Linux x86-64/KVM and Windows/WHP:
[run 42119605760](https://github.com/pichi-vm/dillo/actions/runs/42119605760).
ARM64 Linux and macOS VM jobs remain queued. A further local KVM integration
test confirms guest input through madrillo, kills madrillo, reaps cooperative
dillo exit zero and requires no fabricated retained completion. Its boot-output
scanner initially made the test a stalled viewer; incremental scanning fixed
the harness, and three focused repetitions passed. These are not full
conformance passes, and native macOS/Windows monitor-crash integration remains
unfinished. The full conformance streak remains zero.

The dillo integration is available as [draft PR 92](https://github.com/pichi-vm/dillo/pull/92),
at `61b47fa5e08986f1cf3c615579f4449ca55411f4`. Its current revision passes
Linux x86-64 and Windows VM CI; ARM64 Linux/macOS remain pending. The user asked
which green PRs need review; no madrillo-dependent PR was fully green at that
check, and no unrelated existing PR is being presented as a prerequisite.

Terminal qualification now uses an actual Unix interactive shell: launch a
foreground job, suspend with Ctrl-Z, detach/reattach, verify the stopped job,
resume with fg, interrupt with Ctrl-C, and retain shell exit 37. This passes
locally. The equivalent Windows cmd.exe test starts a foreground ping, detaches,
reattaches, interrupts via the ConPTY input bytes and requires shell exit 37;
native Windows and macOS results are pending.

A Linux native journal test now lowers only its own socket send buffer,
requires a real EMSGSIZE from the kernel, then sends binary output plus 24KiB
metadata through the existing sealed-memfd fallback. The journal's stored bytes
and complete metadata match. It passes locally and is explicitly invoked by
Linux qualification CI; ordinary unit runs mark this host-service-dependent
test ignored. No production logging behavior changed for this test.


At the next review, dillo source revision `61b47fa5` passes the full native VM
matrix (Linux x86-64 and ARM64/KVM, macOS ARM64/HVF, Windows/WHP), plus the
separate three-host console-pipe matrix. PR 92 still has one Linux ARM64 merge
job queued; no queued job has been restarted or counted as passed.

Madrillo `f9ebaf1` passes Linux and Windows qualification, including actual
Windows cmd.exe interrupt/detach and the Linux journal memfd fallback. Its macOS
job was initially cancelled without running any steps. A retry passed the Rust
suite, then exposed a harness error: the switched-account Python probe inherited
a private checkout working directory and failed before attempting the socket.
The probe now runs from its deliberately public scratch directory. The native
macOS peer check still needs a passing execution; this is not a product pass.

Pichi's isolated `feat/madrillo-launch` worktree now includes a native initial
connection, asynchronous bounded framing, retained multi-image workload pins,
run/detached launch, and initial attachment/discovery code. Local image-GC tests
show tag removal retains both appliance and source-image roots; unknown and
incomplete cleanup retain pins, and malformed/partial records stop deletion.
Work is uncommitted and integration remains unfinished: build, stop/logs,
native host qualification and the complete terminal contract are still pending.
Publishing madrillo (currently private) is awaiting user approval so pichi CI
can fetch the types-only dependency without an existing cross-repository token.
The full conformance streak remains zero.


Dillo PR 92 is now ready for review: all source and merge-run checks at
`61b47fa5` are green, including Linux ARM64. The PR was promoted from draft;
it has not been merged. Madrillo qualification at `58be66c9` also passes all
three native hosts ([run 42127125520](https://github.com/pichi-vm/madrillo/actions/runs/42127125520)).
The macOS switched-account probe now reaches the public socket and verifies
peer rejection. These remain qualification checks, not full conformance passes.

Pichi's launch/identity foundation now has local CLI tests for binary stdin,
native exit 37, detached launch, discovery and subsequent retained exit 23.
Its unit/run/rmi/workload checks pass locally, as does the explicitly invoked
native launch/control test. New Rust paths compile for Windows MSVC in the
isolated probe; this does not qualify the full pichi binary or native Windows
execution. Strict workspace Clippy encountered existing registry/storage
warnings; the normal no-deps Clippy run completes and reports no warnings in
the new workload modules/tests.

Workload identity and console state now live in the canonical image cache's
private sibling directory (append `.workloads`), so the build guest's read-only
cache mount does not expose them. Unix reconnect sockets use short `/tmp` names
with both peers checking kernel UID credentials; Windows clients check pipe
ownership. The original preconnected channel remains a transferred capability.
Native client checks, full build integration, stop/logs and signal/resize remain
unfinished. Pichi's changes are local while repository visibility/CI access is
awaiting the user's answer; the full conformance streak remains zero.

The user clarified that console closure defines dillo's lifetime by default.
The opt-in flag was an unnecessary compatibility assumption and is removed in
dillo revision `99c22fe`. Native pipe/terminal hangup detection still handles
unread buffered input; the ordinary stdin producer handles other EOF/read
failures. Pichi no longer injects a lifetime flag. A caller's detach releases
its attachment while madrillo retains the workload console writer.

All 22 local KVM boot tests pass at this revision, including null-input EOF,
ordinary guest input followed by writer closure, and abrupt monitor loss with
cooperative dillo exit zero. The first parallel full run encountered an
unrelated network-port collision; the isolated retry and complete serial run
passed. Console unit tests (39), formatting and Clippy pass locally. PR 92 was
returned to draft, updated and pushed for fresh native-platform qualification;
the older revision's green checks do not qualify this change. Full conformance
remains pending with a streak of zero.

Pichi revision `698b30cf` now shares monitor launch between run and build.
Build transfers its original named-port descriptors at their original numbers;
madrillo does not interpret the guest API. Four local CLI qualification tests
cover binary console input, detached discovery, guest-first build exchange with
OCI import, and invalid-request termination without import. The regression
selection passes 127 unit tests plus 24 CLI tests. The explicit real KVM smoke
also passes: the published Conglobate appliance completes its build, powers off,
and the host imports the output. That smoke used a 120-second monitor runtime
limit to bound failure; normal guest exit completed in about four seconds.

Pichi does not yet provision a workload cgroup, so that run correctly retained
its image pins and output with incomplete descendant-cleanup evidence. Windows
extra-handle inheritance and the other caller gaps remain open. Pichi's branch
is pushed without a PR. Its ordinary CI was explicitly skipped because the
library repository remains private and no cross-repository credential exists.
A manual qualification workflow in this private repository instead checks out
both source revisions and redirects only Cargo's Git fetch transport to the
authenticated local monitor checkout. It does not publish private sources or
change repository visibility. Native caller results are pending.

The user subsequently authorized publishing madrillo. The repository is now
public, confirmed by GitHub visibility and an unauthenticated Git HEAD query.
The temporary local-fetch redirection is removed from caller qualification.
Pichi's normal platform CI now builds the monitor revision pinned by its
library lock entry and supplies the resulting executable to the caller tests;
no cross-repository credential or library runtime dependency is needed.

Dillo PR 92 has been rebased onto current main at `a75f0e3`. All 39 console
unit tests and 24 Linux/KVM boot tests pass locally, including default console
EOF and abrupt monitor loss. The native three-host console matrix is green;
Windows/WHP and macOS/HVF VM CI pass, while Linux jobs were still running or
queued at this checkpoint. The PR remains draft pending current-head checks.

Pichi [draft PR 154](https://github.com/pichi-vm/pichi/pull/154) has been rebased
onto current main and pushed at `307b2760`. Its build path preserves the current
array-based `pichi-launch` protocol, native boot configuration, guest-selected
sources and quiet-build diagnostics. Active build records retain the whole
read-only image cache because the guest selects its own inputs; ordinary run
records retain explicit image pins. Unknown or incomplete cleanup remains
conservative. The matching default-console-lifetime dillo revision is pinned
for both Cargo artifacts and appliance builds.

On this rebased source, 136 unit tests, 27 selected build/run/rmi/workload CLI
tests, and the explicit native initial-channel/reconnect test pass locally.
The earlier native caller run
[42134965662](https://github.com/pichi-vm/madrillo/actions/runs/42134965662)
passed on Linux and macOS, but Windows failed to create the independent monitor
with Access Denied (OS error 5). That failure is unresolved; a denied Job
breakaway is a hypothesis, not an established cause. Earlier appliance smoke
evidence does not qualify the rebased guest protocol. Windows extra-handle
inheritance, caller containment, stop/log presentation and signal/resize work
remain open, as does the macOS C07 scope decision. Full conformance remains
zero; none of these partial qualification runs increments the required streak.

At the user's request, accumulated scratch downloads, probes, logs, bytecode,
conformance output and temporary build directories have been removed. Historical
research deliverables are tracked under `docs/reference/`; active implementation
worktrees and user-owned files are preserved. The final local verification ran
inside an automatically removed temporary directory. CI now removes its monitor
checkout after qualification, including failure and cancellation paths.

Pichi's current draft revision `bf99238b` fixes the packaged host-manifest patch
and passes its commit-message, formatting, license, spec and SRPM checks. Local
packaging verification also applied the patch, trimmed the lock without new
pins, resolved the locked host graph and vendored it successfully; its scratch
copy was removed. Native caller qualification
[42144307625](https://github.com/pichi-vm/madrillo/actions/runs/42144307625)
passes Linux and macOS on this revision. Windows is still running at this
checkpoint, including a comparison of the identical launch from Bash and
PowerShell. No launch flags have been relaxed.

Linux notification qualification now rejects an oversized datagram without
forwarding its valid-looking READY prefix, and exercises a genuinely full
destination receive queue. The harness proves EAGAIN on a fresh sender (an
existing sender can exhaust its own buffer first). Both monotonic timeout and
an explicit control-channel force request remain responsive under that pressure;
the retained and delivered result records agree, failed final delivery is
reported, and the relay socket is removed. These cases, the real systemd
NotifyAccess=main service check and strict Clippy pass locally. Native CI for the
new fixture remains required; full conformance remains zero.

The Windows diagnostic run has now completed. An independent Python process
probe succeeds with detached/new-process-group flags and fails with Access
Denied when only CREATE_BREAKAWAY_FROM_JOB is added, under both Bash and
PowerShell. The native Pichi launch fails the same way; six run CLI tests now
exercise that path and also fail. Windows unit/build/rmi selections pass, but
these do not qualify launch. The source and full job log are in
[job 127651971773](https://github.com/pichi-vm/madrillo/actions/runs/42144307625/job/127651971773).

The caller workflow now also starts a bounded test host through Windows WMI,
which does not inherit the invoking runner's Job Object, and runs the unchanged
caller code there. This is qualification infrastructure only: production still
uses the preconnected inherited channel and requires native Job breakaway.
There is no fallback that leaves monitor lifetime under the caller's Job, and
no production WMI broker. Results from this new test context are pending; a
successful run will not by itself prove every C04 lifetime scenario.

Dillo's unchanged `a75f0e3` source now passes all native VM and console checks.
The first Linux x86 attempt had 15 boot timeouts; the retry passed, with no
source change and no established root cause. [PR 92](https://github.com/pichi-vm/dillo/pull/92)
is ready for review. This does not supply the still-missing macOS/Windows
monitor-crash VM evidence. The earlier notification failure qualification also
passed the three-host monitor CI at `2b8db07`.

C25 diagnostic support now implements conmon's warning default, case-insensitive
levels and aliases, plus Unix `--syslog`. The executable-only syslog dependency
uses the local native service; the bare library still has only serde. Diagnostic
messages never enter workload logs. Unlike conmon's nonfatal LOG_INFO mapping,
syslog priorities reflect each message's severity (trace maps to debug).

One bounded worker isolates diagnostic I/O. It holds at most 32 queued records
of 2 KiB plus a truncation marker; formatting is bounded before enqueueing.
After teardown, a 250 ms flush deadline records delivery failure without
delaying result publication indefinitely. Strict Clippy and all 52 current
local tests pass, with the existing explicit journal qualification still ignored
by the ordinary test command. New native fixtures cover thresholds and aliases,
fatal/invalid-level behavior, workload-log separation, UTF-8 truncation, and a
full stderr socket during timeout. Real Linux syslog delivery passes with PID,
facility, priority and workload separation verified through journal records.
The new syslog check is wired into Linux CI; native results for this revision
remain pending. Full C01-C30 conformance remains zero.

The first WMI-hosted caller test also failed with native Access Denied. Its
host was outside the invoking runner Job, but the harness still ran tests
under Cargo. The pinned Cargo predates
[rust-lang/cargo#17527](https://github.com/rust-lang/cargo/pull/17527), which
adds permission for explicit child breakaway from Cargo's own Job. Microsoft's
[Win32_Process.Create documentation](https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/create-method-in-class-win32-process)
also requires an explicit startup flag to avoid WMI provider Job limits.
The harness now builds the same five test executables with Cargo, runs them
directly from the WMI host, and requests breakaway when creating that host.
These are test-context corrections; no production flags or lifetime guarantees
were relaxed. Native confirmation remains required.

Native confirmation now passes: in [run 42147440403](https://github.com/pichi-vm/madrillo/actions/runs/42147440403),
Windows completed 135 unit tests, 24 applicable CLI tests, and the explicit
native initial-channel/reconnect test under the corrected WMI host. The Unix
Python workload fixture file contains zero Windows tests, so it supplies no
Windows build-channel or logs evidence. Linux and macOS caller selections also
passed. The overall diagnostic workflow remained red because it deliberately
reproduced the ordinary restricted-Job launch failures first. The workflow now
uses an explicit denial test for that context, preserving unknown state and
image pins, and runs the complete caller selection in the permitted context.

Pichi also implements `logs ID` as a bounded snapshot of retained CRI records,
preserving binary stdout/stderr and partial-line fragments without a protocol
extension or replay buffer. At `dd525e9a`, 138 unit tests, 12 workload/removal
CLI tests and the explicit native initial-channel test pass locally. Clippy
completes with existing warnings elsewhere in Pichi. New native caller
qualification is still required. Full conformance remains zero.

Native caller qualification passed on all three hosts at monitor `5f881c4`
and Pichi `b97f82b5` (same source tree as `6e528a89` after commit-message fixes):
[run 42148466526](https://github.com/pichi-vm/madrillo/actions/runs/42148466526).
This resolves the Windows initial-launch qualification issue in a permitted
caller context; the denied-breakaway case also passes without inventing an exit
result or releasing image pins. It does not complete C04's caller-death matrix.

Pichi `caceab40` adds `stop ID`, with console EOF requesting Dillo teardown and a
10-second default grace period before forced cleanup (`--time 0` forces
immediately). Madrillo `5c660ce` permits this administrative EOF while another
authenticated client owns data input. Input bytes still require ownership;
detach still leaves the liveness pipe open. No protocol message was added.
Repeated stop reads retained completion, and workload exit status remains
unchanged. A failed connection or termination deadline rechecks durable state;
missing results remain unknown and pinned.

The pinned monitor passes its native matrix in
[run 42149415700](https://github.com/pichi-vm/madrillo/actions/runs/42149415700).
Pichi's updated native caller matrix passes Linux, macOS and Windows in
[run 42149879242](https://github.com/pichi-vm/madrillo/actions/runs/42149879242),
including clean stop with an active writer, grace expiry, and immediate force.
Local checks passed 139 unit tests, 30 CLI tests, and the explicit native caller
test; Pichi Clippy still reports 23 existing warnings outside this change.
All temporary local build and PR-body files were removed. Pichi PR 154 remains
draft; Dillo PR 92 remains ready with all checks green. C01-C30 full conformance
remains zero, with macOS escaped-descendant containment, Windows exact handle
inheritance, caller containment and remaining native VM qualification unresolved.

The isolated `feat/windows-inheritance-probe` branch qualifies Firehazard at
`1b5818d8fa6c489addb8cbb133a62ab917cc1715` through safe owned Rust. At probe
revision `d9e3ae2`, all three native jobs passed in
[run 42151573930](https://github.com/pichi-vm/madrillo/actions/runs/42151573930).
The actual Windows fixture preserved an extra pipe's numeric handle, separate
binary stdio, exclusion of an unlisted inheritable writer, atomic Job membership,
native exit 37 and observed Job emptiness. The first run rejected an incorrectly
initialized extended-startup structure size; that probe bug was fixed.

This is not a selected replacement for production launch. Firehazard's safe
handle-list API needs its typed borrowed handles: converting Madrillo's existing
inherited `FileDescriptor` to a standard `File` duplicates it and changes the
number. Safe adoption preserving the original number is still unqualified.
Its synchronous-stdio types also need composition with existing passthrough
handles; pseudoconsole creation requires unsafe code, and the command-line
helper's trailing-backslash behavior needs separate qualification. Cross-checking
on Linux fails on the dependency's Windows-style source paths; native Windows
compilation and execution passed. The experimental dependency and fixture remain
on that branch only, with no production dependency or launch changes.

RightKit 0.5.7 exposes exact raw-handle inheritance and suspended creation, but
its contained spawn always adds a kill-on-close Job. It therefore cannot replace
the pipe path's cooperative monitor-loss behavior as-is. No dependency code or
unsafe shim was copied. Full conformance remains 0/3.

macOS diagnostic qualification exposed a real delivery gap: the Unix socket
writer returned success, but neither legacy logs nor unified logging contained
the record in [run 42152439560](https://github.com/pichi-vm/madrillo/actions/runs/42152439560).
The macOS backend now uses Apple's unified logging through the safe `oslog`
0.2.0 API (MIT, executable-only, default logger feature disabled). Linux retains
its qualified syslog backend; the bare library still depends only on serde.
The native fixture requires the error message, severity, exact PID, executable,
subsystem and category, and keeps workload stderr separate. All three hosts
passed at `04adf21` in [run 42153084016](https://github.com/pichi-vm/madrillo/actions/runs/42153084016).
That initial macOS fixture ran as root; the workflow now runs it as the ordinary
runner user. Unprivileged delivery and observation passed with UID 501 at
`c79965d` in [run 42153718573](https://github.com/pichi-vm/madrillo/actions/runs/42153718573),
with all three native jobs green. An earlier unprivileged attempt saw the
workload terminate with SIGKILL instead of exit 37; the cause of that particular
termination is not established, and the fixture now retains native failure
diagnostics. System filtering/retention still apply; submitting a diagnostic is
not a durable-storage acknowledgement. Full conformance is 0/3.

A separate stop/resume regression exposed an actual macOS supervision bug in
[run 42153937972](https://github.com/pichi-vm/madrillo/actions/runs/42153937972/job/127683475541):
the monitor treated a stopped-child event returned by `waitid` as completion
and killed the workload. Unix exit observation now requires a terminal status,
preserving stopped/continued workloads. The regression stops a running echo
workload, requires the monitor and unpublished result to survive, resumes input,
and verifies normal EOF/exit. Local strict Clippy, 12 control tests and 12 core
tests pass; native verification of the fix is pending. This does not establish
the cause of the earlier isolated SIGKILL.

The stop/resume regression passed on macOS at `86b8a87`; that run still failed
the separate launch diagnostic fixture. All three native jobs passed at
`ac1e2ec` in [run 42154440189](https://github.com/pichi-vm/madrillo/actions/runs/42154440189).
Expanded macOS launch stress then reproduced the separate SIGKILL and captured
its native crash report in [run 42155068239](https://github.com/pichi-vm/madrillo/actions/runs/42155068239/job/127687148243).
It is a libSystem child-at-fork abort (`os_once_t is corrupt`), with
`_notify_fork_child` inheriting notification initialization in progress on the
diagnostic worker. No workload code had executed. Apple's
[notification implementation](https://github.com/apple-oss-distributions/Libnotify/blob/main/notify_client.c)
and the [Go runtime's corresponding workaround](https://go.googlesource.com/go/+/master/src/runtime/sys_darwin.go)
document this initialization/fork interaction.

Native logger construction now completes before starting its worker and before
workload launch. All 128 ordinary-user macOS diagnostic launches passed at
`0b8da07`, with Linux and Windows also green in
[run 42155446764](https://github.com/pichi-vm/madrillo/actions/runs/42155446764).
The stress fixture fails on the first failed launch; it does not retry failures
away. Full C01-C30 conformance remains 0/3.

The user approved conmon's native process-group contract for macOS on
2026-11-05. C07 now covers children and grandchildren remaining in Madrillo's
managed workload group. Dillo keeps helpers in that group and owns shutdown
of deliberate exceptions; changed groups/sessions and shell job-control groups
remain outside this macOS cleanup guarantee. Linux/Windows scope requirements
are unchanged. The monitor now observes group disappearance after termination
and reaping before reporting complete macOS cleanup. It sends no further
termination signal after reaping, avoiding a reused numeric process-group ID.

New fixtures exercise grandchildren and concurrent child creation during
leader exit, timeout and explicit force, with pipes and terminals. A macOS
fixture explicitly demonstrates a new-session helper surviving managed-group
cleanup and observes its bounded natural exit before removing test files.
Linux strict Clippy and the core fixtures pass; native macOS qualification of
this contract is pending. C07 and the full conformance streak remain pending.

Caller-group loss exposed a same-turn input ownership bug on Linux and macOS
in [run 42157026396](https://github.com/pichi-vm/madrillo/actions/runs/42157026396).
Connections now release input ownership when EOF is observed, before processing
attachments accepted in that polling turn. A deterministic socket regression
passes locally, and the existing detach test now reconnects without its masking
delay. Native caller retesting is pending.

Linux and macOS caller-loss/reconnection now pass in
[run 42158301121](https://github.com/pichi-vm/madrillo/actions/runs/42158301121).
The new native macOS force fixture exposed `EPERM` while observing a dying
process group in [run 42158398325](https://github.com/pichi-vm/madrillo/actions/runs/42158398325).
[XNU's killpg1 implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_sig.c)
returns that error when the group exists but its iterator finds no eligible
non-zombie member. Force/observation now defer that ambiguous result to the
bounded group-disappearance check. EPERM never proves successful cleanup:
only ESRCH does, and a group remaining after the deadline is incomplete.
Native qualification of this adjustment is pending.

All three native caller jobs are green in
[run 42158301121](https://github.com/pichi-vm/madrillo/actions/runs/42158301121),
including immediate reconnection after Unix caller-group loss and Windows
denied/permitted Job breakaway. Native distribution preparation now builds
the release binary and verifies the source package's default library build,
then bundles dependency license texts/inventory and Rust library notices.
The local Linux distribution passed with 81 Cargo dependency entries; its
temporary target and archive were removed. Native macOS/Windows packaging
is still pending. This does not increment full conformance.

The approved macOS process-group contract passes native qualification at
`d8db137` in [run 42158733952](https://github.com/pichi-vm/madrillo/actions/runs/42158733952),
with Linux and Windows also green. The macOS fixtures require complete cleanup
for immediate leader exit, ordinary descendants, grandchildren, concurrent
forks, timeout and force, and demonstrate the deliberate new-session boundary.
This resolves the macOS scope decision; full cross-platform C07 remains pending.

A repeated Windows real-VM crash qualification failed before the monitor was
killed: its initial attachment closed while the log retained the guest input
acknowledgement, in [Dillo run 42158450563](https://github.com/pichi-vm/dillo/actions/runs/42158450563/job/127698098101).
Inspection found that each flush issued only one Windows 8 KiB write, even
with a writable peer, leaving a growing encoded backlog. Flush now drains its
bounded pending bytes until actual backpressure. A 256 KiB startup-burst
fixture requires complete delivery and subsequent input/EOF on the same
attachment. Local strict Clippy and all 14 control tests pass; native Windows
and real-VM retesting are pending. Queue limits remain unchanged.

Repeating native qualification at `bdbec9e` exposed a macOS fork race:
[run 42158989862](https://github.com/pichi-vm/madrillo/actions/runs/42158989862/job/127699857045)
timed out after force and left a fixture descendant. XNU's
[pgrp_iterate](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_proc.c)
collects a membership snapshot before delivering signals. macOS force now
repeats group SIGKILL while the unreaped leader still reserves its group ID,
until no signalable member remains or the deadline expires. Only after that
does it reap the leader and observe group disappearance. No termination signal
is sent after reaping. The force fixture now executes eight iterations of each
pipe/terminal and grandchild/forking case. Native retesting is pending.

Windows packaging built and verified the binary/library and collected its
licenses, then rejected a dependency file timestamp earlier than ZIP's 1980
epoch. Packaging now uses ZIP's documented timestamp clamping, preserving file
contents; a pre-1980 local fixture verifies that behavior. Native retesting is
pending. Linux produced its native archive successfully.

The repeated macOS force cases pass at `95d010b`, but the startup-burst
fixture exposes an undersized pending-output budget on macOS: 256 KiB of
encoded JSON represents only about 64 KiB of console bytes. The fixture's
actively draining client can fill that budget during a scheduling burst.
The per-client bound is now the existing 1 MiB maximum-frame budget, allowing
the 256 KiB console fixture without adding scheduling, replay or buffering
protocols. Stalled viewers still disconnect independently at the bound;
control/timeout/logging behavior and the 32-client cap remain unchanged.
Native qualification is pending.
