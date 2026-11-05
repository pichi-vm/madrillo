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
- CLI scaffolding with version/API identity; monitoring is not implemented yet.
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
  execution is being checked in CI; cgroup placement is not yet qualified.
- `process-wrap` 10.0.1's Windows JobObject uses suspended creation/assignment
  and requests `make_job_object(..., false)` without mandatory kill-on-close.
  This is promising for dillo cleanup, but combined PTY/inheritance behavior
  and actual native Windows execution remain unqualified.

Next: complete inherited resource and scoped terminal experiments, implement
bounded framing/core supervision, and keep required missing behaviors failing
the conformance gate. macOS arbitrary-descendant containment remains unresolved.
