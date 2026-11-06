# Madrillo simplification review

Twelve sequential passes on the architecture and CLI, completed 2026-11-05.
This is a historical record of those passes, not the current scope. The user
subsequently required conmon parity for functions removed here, including
descendant supervision and logging, and reopened terminal support. The current
proposal is in [Terminal architecture](TERMINAL-ARCHITECTURE.md); the
[revised conmon requirements](CONMON-COVERAGE.md) supersede conflicting decisions
and qualification lists below. The twelve pass results themselves are preserved.
This review changes documentation; it creates no madrillo implementation.

The governing rule is deletion: retain a mechanism only when a required
behavior needs it. The design at the end of these passes had no start handshake, bootstrap channel,
replay service, logs RPC, readiness service for detached run, or host PTY layer.

| Pass | Focus | Result and preserved behavior |
| --- | --- | --- |
| 1 | Scope | Removed speculative host PTY/ConPTY support, runtime plugins, remote API, restart, and descendant management. Kept generic pipe-connected children and the guest's own terminal. |
| 2 | Package and library | Kept one package with a types-only library and private executable modules. Removed any implied client SDK, transport helpers, or runtime dependency in the library. |
| 3 | Launch lifecycle | Collapsed automatic and gated launch into one path. This intermediate pass still used `Start`; pass 6 removed that exchange entirely. |
| 4 | Native CLI and stdio | Reused ordinary child stdin/stdout for the primary console and native argv/environment/cwd. Reduced stdin configuration to one flag. Retained separate UART and diagnostic capture. |
| 5 | Extra resources | Replaced the broad launch-resource contract with two concrete roles: capture an output and pass an opaque resource. Kept the locked inherited build pipes and exposed the safe-API qualification requirement. |
| 6 | Startup protocol deletion | Established initial attachment through the pipes used to spawn madrillo. Deleted `Start`, wait-for-start modes, abandoned-start deadlines, separate bootstrap transport, detach requests, and negotiation machinery. |
| 7 | Logs | Made files the only history and pichi their reader. Deleted logs RPC, drivers, modes, indexes, secondary backlog, and exact replay/resume. Kept bounded rotation, following, and visible loss. |
| 8 | Input | Reduced input to one pipe and one connection owner. Removed policy variants, leases, takeover, and per-source input machinery. Kept explicit EOF, release, reconnect, and monitor-loss semantics. |
| 9 | Process lifetime and result | Used one owned child handle, one reap, and one retained result. Removed stale-PID termination/recovery and double-fork machinery. Kept native status and a finite output drain. |
| 10 | Monitor loss | Kept console loss as dillo's cleanup trigger. Removed duplicated signal/watchdog/heartbeat designs. Preserved cleanup under full input buffers as a native qualification requirement. |
| 11 | Pichi CLI | Shared run/attach behavior and stream selection. Removed mandatory dillo readiness from `run -d`; retained direct supported resize/shutdown and existing guest-initiated build readiness. |
| 12 | Whole-design audit | Replaced accumulated proposal text with one consistent final design, moved original research behind it, removed superseded behavior, and mapped essential functionality to acceptance cases. |

## Mechanisms that still have a job

- **Native reconnect endpoint:** needed after the original pichi exits. Initial
  stdio alone cannot provide later attachment.
- **Small framed protocol:** carries multiple byte sources and host results
  without confusing arbitrary console bytes with control data. The initial
  connection and later attachments use the same format.
- **Two extra-resource roles:** UART must retain source identity, and the build
  contract requires inherited pipe ends. They do not justify a general resource
  broker or a second launch specification.
- **Input ownership and explicit EOF:** multiple observers must not become
  competing writers, and client loss must not close dillo's console.
- **Bounded queues and finite drain:** prevent a slow client, storage failure,
  or descendant retaining an output pipe from consuming unlimited resources or
  keeping completion pending forever.
- **Retained log/result types:** pichi must read output and host completion
  after madrillo exits. The implementation still belongs in the executable.

## Essential behavior retained

The final acceptance table covers foreground/detached launch, CLI independence,
live reattachment, retained/followed logs, native exit status, explicit force
stop, input arbitration, terminal restoration and resize, separate console/UART/
diagnostic streams, and controlled dillo cleanup after monitor loss. Child APIs
remain direct and guest command execution remains out of scope.

The separate repository, bare library, `forbid(unsafe_code)`, no-panic handling
of operational failures, and native Linux/macOS/Windows MSVC requirements remain.
The existing build pipe inheritance, CBOR exchange, guest-first ordering, and
image-defined device model are preserved.

## Verification and remaining native qualification

All twelve intermediate revisions were saved and checked to be distinct.
Document references, local links, code fences, and whitespace pass checks;
`cargo fmt --check` passes. The original proposal portion contained 5,477 words;
the revised design, including its overview, contains about 3,250. Original
Docker/Podman research and source observations remain in the same document.

A Linux pipe probe filled 65,536 bytes, closed the writer, and observed `POLLHUP`
while all bytes were still unread. This supports the Linux OS mechanism; it
does not qualify dillo's implementation or either other platform.

Two native details still need proof before implementation:

1. Safe ownership and exact forwarding of additional inherited resources on
   every host. The checked `command-fds` receiver example requires unsafe
   initialization, so it does not satisfy madrillo's rule by itself.
2. Dillo's detection of console loss when guest input consumption stops and
   buffers are full, on all three platforms. An empty-pipe EOF test is
   insufficient, and no fallback heartbeat/control protocol has been invented.

These are explicit qualification gates. This review does not claim native
madrillo runtime tests have passed or relax the requirements to bypass them.
