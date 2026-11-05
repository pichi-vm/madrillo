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

The private remote is https://github.com/pichi-vm/madrillo. Native foundation CI
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
