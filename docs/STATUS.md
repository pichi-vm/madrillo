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
