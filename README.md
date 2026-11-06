# Madrillo

The mother armadillo: a native process monitor inspired by conmon.

This repository is under construction. The [implementation plan](PLAN.md)
defines the full scope, success criteria, and required three consecutive clean
conformance passes. No feature completeness or native-platform conformance is
claimed yet.

Madrillo is one Rust package: a minimal shared-types library and a complete
monitor executable. All owned Rust targets forbid unsafe code. Pichi and dillo
are the first integration, while the monitor remains independent of VM APIs.

`--capture NAME RESOURCE` transfers an inherited pipe read end to the monitor.
It records raw bytes in `STATE/stream-NAME.log`; clients can select `NAME` in
their attachment. The caller separately passes the write end to the workload
(Unix `--inherit NUMBER`) using the workload's own arguments. No contents or
arguments are interpreted. Captures are explicit additional logs even with
`--log-path none`; size/rotation/sync and allowlist options apply to them too.
Their byte limits are exact; stdout/stderr retain conmon's whole-record CRI
behavior. Capture resources count toward the 64-resource bound and must have
exclusive read ownership. Windows currently accepts synchronous pipe read
handles; exact workload inheritance on Windows remains unfinished.

On Windows, the state directory's existing parent must grant private,
inheritable access to the invoking user (SYSTEM/Administrators are also
trusted). Madrillo validates that parent before creating state; it rejects a
shared parent instead of exposing retained data during an ACL update.

The caller must give the monitor an independent native lifetime. On Windows,
Pichi requests `CREATE_BREAKAWAY_FROM_JOB`; a containing Job must permit that
operation. A launch that is denied does not fall back to keeping the monitor
inside the caller's Job. The current hosted Windows runner denies breakaway
under both Bash and PowerShell; caller qualification also exercises a separate
test host outside that Job. This remains an open qualification requirement.

Unix listeners check each connecting process's effective UID against the
monitor's user before accepting requests or delivering output. This also
protects custom socket paths on hosts that do not enforce socket permission
bits. An inherited initial connection remains a caller-transferred capability.

`CloseInput` is an administrative request available to any authenticated peer,
like `Terminate`. It closes pipe input after already accepted bytes drain,
even when an attachment still owns input. Only the owner may send input bytes;
detach and ordinary client EOF still release ownership without closing the
workload's input. Terminal input cannot be half-closed.

Monitor diagnostics go to stderr. `--log-level` defaults to `warn`, with
`error`, `info`, `debug`, and `trace` also supported. Names are case-insensitive;
conmon's `warning`, `fatal`, and `panic` aliases are accepted. Unix `--syslog`
additionally sends diagnostics to the local syslog service as `madrillo`, with
the monitor's PID and user facility. Severity follows the message level (trace
uses syslog debug). On macOS this uses Apple's unified logging instead, under
subsystem `org.pichi.madrillo` and category `monitor`. Error, warning, info and
debug/trace map to native error, default, info and debug respectively; the
message also includes the original level. View these with Console or
`log show --predicate 'subsystem == "org.pichi.madrillo"' --info --debug`.
Native filtering and retention apply; submitting a diagnostic does not
acknowledge its storage by the system service. Workload logging remains
configured separately by `--log-path`; workload bytes are never sent through
the diagnostic logger.
With passthrough, both monitor diagnostics and workload stderr use the caller's
stderr; `--log-level error` suppresses non-error monitor messages there.

Diagnostics use one worker with 32 queued messages, each limited to 2 KiB plus
a truncation marker. A blocked sink cannot stop supervision. After workload
teardown, flushing waits at most 250 ms; dropped messages, sink failures, and
an expired flush deadline are recorded in `result.json`. Diagnostics remain
best effort across monitor loss. Errors after result publication, such as an
exit-command failure, cannot rewrite the workload result.
