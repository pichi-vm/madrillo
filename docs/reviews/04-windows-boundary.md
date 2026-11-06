# Private Windows boundary review

The user approved this exception on November 6, 2026 to unblock exact opaque
HANDLE inheritance and ConPTY scope accounting. The library remains bare and
forbids unsafe code. Only `src/windows.rs` may use unsafe code in the executable;
`tools/check_library.py` checks that boundary. No unsafe shim is exposed to callers.

## Ownership and native calls

Incoming numeric resources are validated before opening handles or starting
workers, then exclusively adopted through `FileDescriptor`. Their numeric values
remain unchanged. Inheritance flags are cleared at adoption and temporarily set
only for selected workload resources. A guard restores their prior flags on
every return path. The explicit attribute list contains only selected resources
and the privately owned stdio handles. Control and held resources are excluded.
All originals passed to the child are dropped after creation, allowing real EOF.

Native process creation combines the handle list, Job list and optional
pseudoconsole attribute. Job membership is established before child code runs.
Attribute storage is 16-byte aligned; backing arrays and native owners remain
alive through creation and list deletion. The process and thread handles are
adopted exactly once after success. Creation errors close all owned resources.
See Microsoft's [attribute contract](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute).

The owned Job, rather than a recycled PID or a process snapshot, identifies the
scope for force termination and `ActiveProcesses == 0` observation. Completion
waits have a deadline and preserve an explicit incomplete result on failure.
Pipe workloads retain cooperative monitor-loss shutdown. Terminal Jobs use
kill-on-close. Leader exit code is read only after the process handle signals,
preserving all 32 bits and genuine exit code 259.

ConPTY owns synchronous input/output pipes. One pre-created closer thread takes
exclusive ownership of HPCON under the resize mutex. It closes outside the
supervisor/capture threads so legacy blocking close can coexist with output
drain. Constructor failures close the pipe endpoints before synchronous close;
no workload exists on that path. No raw handle is shared using an unsafe Send
implementation. See Microsoft's [ConPTY lifecycle](https://learn.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session).

Before console creation, `SetConsoleCtrlHandler(NULL, FALSE)` clears inherited
Ctrl-C suppression. This flag otherwise propagates from a new-process-group
launcher through Madrillo into the independent ConPTY workload. The call has
no pointer or callback lifetime. The detached monitor still uses authenticated
control for cancellation. This follows [Windows Terminal's correction](https://github.com/microsoft/terminal/pull/5472)
and the [documented inheritance contract](https://learn.microsoft.com/en-us/windows/console/setconsolectrlhandler).
The native shell fixture starts its monitor in a new process group and must
interrupt a foreground program after detach/reattach, then preserve shell exit
37. Qualification also runs it through Bash and redirected Python, the launcher
environment that exposed the defect.

Native text remains UTF-16, including unpaired surrogates. Argument quoting
uses Windows CRT escaping without shell interpolation; batch scripts require an
explicit interpreter. Environment keys use Windows ordinal case-insensitive
comparison. Embedded NULs and invalid sizes fail before launch. Native executable
search follows `CreateProcessW`; the private Command builder inherits the parent
environment and supplies explicit updates, never `env_clear`.

## Required evidence

The Windows test `exact_handles_compose_with_pipe_and_conpty_and_release_before_hook`
launches the monitor through the native boundary, passes an opaque binary pipe
through the second hop, excludes a held resource, and requires EOF while the
exit callback remains active. Both launch modes require complete cleanup and
the original exit code. Existing suites cover native non-Unicode text, full exit
codes, spawn failure, descendant races, forced termination, timeout, terminal
resize, drain, stalled clients and input, and retained results.

Linux tests and strict Linux/Windows cross-target checks are necessary regression
checks. Actual native Windows execution and the three full conformance rounds
remain required; cross-compilation does not establish OS behavior.
