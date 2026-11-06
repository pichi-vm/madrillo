# Madrillo

The mother armadillo: a native process monitor inspired by conmon.

This repository is under construction. The [implementation plan](PLAN.md)
defines the full scope, success criteria, and required three consecutive clean
conformance passes. No feature completeness or native-platform conformance is
claimed yet.

Madrillo is one Rust package: a minimal shared-types library and a complete
monitor executable. The library and qualification targets forbid unsafe code.
The executable denies unsafe code except for its private, audited Windows
OS boundary, approved to compose exact handle inheritance, Jobs and ConPTY.
See the [boundary review](docs/reviews/04-windows-boundary.md). Pichi and dillo
are the first integration, while the monitor remains independent of VM APIs.

CI uses public Linux, macOS and Windows runners with ordinary child-process
fixtures. It does not build or execute the Dillo VM binary or require hardware virtualization.
Pichi's caller and build-channel contracts are checked through the real CLI
without booting a guest. See the [CI scope review](docs/reviews/06-public-ci.md).

On Linux, Madrillo becomes a subreaper before launching the workload. After
leader termination it adopts, terminates and reaps remaining descendants,
including helpers in different groups or sessions. It validates child ownership
and signals through pidfds; only a kernel wait reporting no children establishes
completion. This requires native pidfd/wait support, readable procfs, and
permission to signal the descendants. A supplied `--cgroup` additionally places
the workload before execution and observes OOM evidence. V2 uses atomic subtree
termination and `memory.events`; v1 uses subreaper cleanup, native OOM eventfd
registration and the final `memory.oom_control` kill counter. The caller retains
the cgroup until completion. Missing observation support (including v1 event
registration on PREEMPT_RT), permissions or failed cleanup remain explicit in
the result.

Linux OOM observation also creates `state-dir/oom` while the workload is still
running, matching conmon's persistent marker. Native inotify/eventfd notifications
drive observation; final counters are checked again at completion. The marker
survives monitor loss and reports an observed workload-scope event, not proof
that its leader died from OOM. Missing completion remains unknown. Marker write
or synchronization failures are retained as monitor errors when completion can
be published; evidence not written before a crash cannot be promised.

File logs synchronize by default; `--no-sync-log` disables that step, not result
publication. A retained result is written to a temporary file, synchronized and
renamed without overwriting another result. Unix also synchronizes the containing
directory. A publication failure returns a monitor error and withholds the final
completion message and exit command. If directory synchronization fails after
the rename, the valid result remains available, but its presence alone does not
prove synchronization succeeded. Result data records workload facts, not an
acknowledgement that an exit callback ran. Filesystem and device guarantees still
apply; Windows rename publication does not provide a directory-flush or universal
power-loss guarantee.

On macOS, Madrillo manages the workload process group it creates. Children and
grandchildren must remain in that group for Madrillo to guarantee their cleanup
after leader exit, forced stop or timeout. A program that creates another group
or session owns shutdown of the processes it moves there. Interactive shells
often create separate job-control groups, so terminal support does not extend
this cleanup guarantee. Dillo keeps its helpers in the managed group; any
deliberate exception remains Dillo's responsibility. This is the approved
conmon process-group contract.

`--capture NAME RESOURCE` transfers an inherited pipe read end to the monitor.
It records raw bytes in `STATE/stream-NAME.log`; clients can select `NAME` in
their attachment. The caller separately passes the write end to the workload
(`--inherit NUMBER`) using the workload's own arguments. No contents or
arguments are interpreted. Captures are explicit additional logs even with
`--log-path none`; size/rotation/sync and allowlist options apply to them too.
Their byte limits are exact; stdout/stderr retain conmon's whole-record CRI
behavior. Capture resources count toward the 64-resource bound and must have
exclusive read ownership. Windows accepts synchronous pipe read handles.
Explicit inheritance preserves the numeric handle through native process
creation, including ConPTY workloads. Monitor-only resources are excluded.

On Windows, the state directory's existing parent must grant private,
inheritable access to the invoking user (SYSTEM/Administrators are also
trusted). Madrillo validates that parent before creating state; it rejects a
shared parent instead of exposing retained data during an ACL update.

The caller must give the monitor an independent native lifetime: a fresh session
on Unix, or separation from the caller's console and Job on Windows. Captured
pipe and terminal workloads can then outlive caller exit, crash and terminal
closure, with later clients reconnecting to the monitor. On Windows,
Pichi requests `CREATE_BREAKAWAY_FROM_JOB`; a containing Job must permit that
operation. A launch that is denied does not fall back to keeping the monitor
inside the caller's Job. Native caller qualification exercises both denied
breakaway and permitted caller Job loss, using a separate test host outside
the runner's Job. It also closes the caller's actual console window and checks
that the surviving monitor/workload accepts reattachment and binary input.

Unix listeners check each connecting process's effective UID against the
monitor's user before accepting requests or delivering output. This also
protects custom socket paths on hosts that do not enforce socket permission
bits. An inherited initial connection remains a caller-transferred capability.

`CloseInput` is an administrative request available to any authenticated peer,
like `Terminate`. It closes pipe input after already accepted bytes drain,
even when an attachment still owns input. Only the owner may send input bytes;
detach and ordinary client EOF still release ownership without closing the
workload's input. Terminal input cannot be half-closed.

On Unix, `SIGINT`, `SIGTERM`, `SIGHUP` and `SIGQUIT` received by Madrillo are
forwarded to the workload process group. Madrillo continues reaping and log/result
finalization; a workload's signal handler may complete normally. On Windows,
the detached monitor uses the authenticated `Terminate` request for forced
workload cancellation. Killing the monitor itself is monitor loss and cannot
publish completion. Console control bytes are separate: a Ctrl-C byte sent to
a host PTY/ConPTY is interpreted by its terminal and foreground program. Dillo
uses pipes and handles console loss through its own shutdown contract.

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
Passthrough gives the workload the caller's native stdio resources directly.
Their lifetime remains with the actual pipe or terminal owner; closing them can
end workload I/O or terminate the workload. Madrillo does not capture or restore
those streams for later attachment.

Diagnostics use one worker with 32 queued messages, each limited to 2 KiB plus
a truncation marker. A blocked sink cannot stop supervision. After workload
teardown, flushing waits at most 250 ms; dropped messages, sink failures, and
an expired flush deadline are recorded in `result.json`. Diagnostics remain
best effort across monitor loss. Errors after result publication, such as an
exit-command failure, cannot rewrite the workload result.

Build the executable with `cargo build --locked --release --features cli`.
A default library build uses only the shared types and serde.

For a foreground launch on Linux or macOS, choose a state directory that does
not exist yet:

```sh
./target/release/madrillo --state-dir ./hello-state -- printf 'hello\n'
cat ./hello-state/console.log
cat ./hello-state/result.json
```

On Windows, use a new directory below your private application-data directory:

```powershell
$madrilloState = Join-Path $env:LOCALAPPDATA ('madrillo-' + [guid]::NewGuid())
.\target\release\madrillo.exe --state-dir $madrilloState -- cmd.exe /d /c echo hello
Get-Content (Join-Path $madrilloState 'console.log')
Get-Content (Join-Path $madrilloState 'result.json')
```

The workload starts immediately and writes to the retained CRI log. Read its
exit status and cleanup outcome from `result.json`; the monitor's own exit code
reports monitor success or failure. `--timeout 30` limits workload runtime to
thirty seconds. `--terminal --size 24x80` gives the child a native PTY/ConPTY;
the caller handles interactive input, attachment and resize through the control
channel.

A caller such as Pichi creates the independent monitor lifetime described above.
Starting with no attached client records output immediately. An already-connected
inherited `--control` channel receives the first live output without a client
handshake. Later clients use `--listen`, or the default `STATE/control.sock` on
Unix and `\\.\pipe\madrillo-PID` on Windows. Attachments receive live output,
and retained logs provide history. Detaching
releases input ownership while the monitor keeps the workload's input open.
The optional `--start-gated` waits for explicit caller authorization when external
placement must finish before workload execution. See `madrillo --help` for the
complete option list.

For a native distribution, run `python3 tools/package.py /path/to/output` from a clean
checkout. The archive contains the executable, Cargo-verified source package,
README, project license, dependency license texts and inventory, and Rust
library notices. A separate SHA-256 file identifies the archive. Native CI
retains these archives for review.

For Fedora/Copr, `make -f .copr/Makefile srpm outdir=/path/to/output`
packages the committed source and vendors the locked dependencies in a temporary
directory. Pass `DNF=true` when the build tools are already installed. Rebuild
the resulting source RPM with `rpmbuild --rebuild`; that phase is offline and
installs only the `madrillo` executable, documentation and license inventory.
Pichi's RPM requires this separate `madrillo` package alongside `dillo-vm`.
