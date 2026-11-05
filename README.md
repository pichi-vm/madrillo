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
