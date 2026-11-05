# Implementation status

Goal active. Three sequential plan reviews are complete. Full conformance:
**0 of 3 consecutive clean passes**. All C01-C30 entries remain pending.

Implemented so far:

- Standalone repository, pinned Rust toolchain and dependency lockfile.
- Types-only default library; executable and qualification dependencies are gated.
- Protocol/result type draft and explicit unsafe prohibition on owned targets.
- Bounded framing with six passing tests for malformed/truncated/oversized
  input, fixed version, binary payloads, and bounded encoding without partial
  oversized transmission. This component is not yet a running monitor.
- Captured pipe launch, native argv, retained stdin writer, monotonic timeout,
  bounded capture/log workers, binary CRI logs, file caps/rotation and fsync,
  private state directories and atomic result publication. Attachment and
  terminal operation are not implemented in the monitor yet.
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
Both behaviors have executable tests. Windows inherited control is explicitly
unsupported until its ownership/inheritance path is qualified.

The executable uses Unix process groups as an implementation step and explicitly
records incomplete cleanup guarantees in its result. Linux cgroup containment
is not implemented. Windows uses process-wrap's suspended Job Object assignment;
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
ConPTY remains explicitly unsupported. Unix termination-signal forwarding has a
real process test. Reaping and retained-state fsync waits are now bounded.

The pinned conmon source has been built locally as a test oracle. The same
binary fixture produced matching CRI binary payloads and exit 37. The committed
CI harness independently rebuilds the pinned archive and verifies its digest
before differential tests; twelve comparisons passed in that Linux CI run.

Next: native CI of core launch, live attachment/input/control, inherited initial
connection, bounded finalization, native scopes and terminals. macOS arbitrary
descendant containment remains unresolved; a scope clarification is pending.
