# Final implementation and conformance review

All C01–C30 requirements passed three consecutive complete native rounds in
[run 42232080329](https://github.com/pichi-vm/madrillo/actions/runs/42232080329).
The verified Madrillo revision is `defe4b7468d0e1f7c7c22c793f254d52ae73a2a4`,
with Pichi `b8ddfbd3b45be53a0a067b72119866ade1cbe8bc`. This document records
those results; the follow-up changes are documentation only.

| Full round | Linux | macOS | Windows MSVC | Verified streak |
| --- | --- | --- | --- | --- |
| `42232080329-1-1` | 30 cases passed | 18 cases passed | 14 cases passed | 1 |
| `42232080329-1-2` | 30 cases passed | 18 cases passed | 14 cases passed | 2 |
| `42232080329-1-3` | 30 cases passed | 18 cases passed | 14 cases passed | 3 |

The `conformance-evidence` artifact contains the verified ledger, all nine native
reports, exact command logs, distributions and pinned conmon oracle artifacts.
The final audit independently checked all 186 command logs against their hashes,
commands and positive execution markers; source identities, native compiler
platforms, unique invocation IDs and ledger entries matched. All nine native
distribution checksums, executable/library payloads and dependency license
inventories were also verified. The complete requirement-to-case mapping remains
in [CONFORMANCE.json](../../CONFORMANCE.json).

| Requirement | Behavior | Required native hosts | Full rounds |
| --- | --- | --- | --- |
| C01 | Bare library and scoped native safety | linux, macos, windows | 3/3 |
| C02 | Native launch | linux, macos, windows | 3/3 |
| C03 | Startup and first output | linux, macos, windows | 3/3 |
| C04 | Independent lifetime | linux, macos, windows | 3/3 |
| C05 | Reconnection | linux, macos, windows | 3/3 |
| C06 | Input ownership | linux, macos, windows | 3/3 |
| C07 | Workload descendants | linux, macos, windows | 3/3 |
| C08 | Reaping and result | linux, macos, windows | 3/3 |
| C09 | Terminal mode | linux, macos, windows | 3/3 |
| C10 | Extra resources | linux, macos, windows | 3/3 |
| C11 | CRI file logs | linux, macos, windows | 3/3 |
| C12 | Log destinations | linux, macos, windows | 3/3 |
| C13 | Log controls | linux, macos, windows | 3/3 |
| C14 | Pressure and failures | linux, macos, windows | 3/3 |
| C15 | Durability | linux, macos, windows | 3/3 |
| C16 | Notification relay | linux, macos, windows | 3/3 |
| C17 | Socket activation | linux, macos, windows | 3/3 |
| C18 | Runtime timeout | linux, macos, windows | 3/3 |
| C19 | Placement | linux, macos, windows | 3/3 |
| C20 | OOM evidence | linux, macos, windows | 3/3 |
| C21 | Monitor OOM protection | linux, macos, windows | 3/3 |
| C22 | Held resources and cleanup | linux, macos, windows | 3/3 |
| C23 | Exit command | linux, macos, windows | 3/3 |
| C24 | Monitor loss | linux, macos, windows | 3/3 |
| C25 | Protocol and diagnostics | linux, macos, windows | 3/3 |
| C26 | Pichi integration | linux, macos, windows | 3/3 |
| C27 | Locked build channel | linux, macos, windows | 3/3 |
| C28 | Crash reconciliation | linux, macos, windows | 3/3 |
| C29 | Native distribution | linux, macos, windows | 3/3 |
| C30 | Monitor termination events | linux, macos, windows | 3/3 |

The final macOS monitor-loss case confirms why the earlier fixture failed:
input-pressure setup took 5.38 and 6.51 seconds in two rounds, exceeding its old
five-second workload timeout. The corrected fixture separates that setup budget
from the unchanged five-second post-crash cleanup deadline. All rounds require
actual backpressure, an independently live monitor before crash injection,
cooperative leader/helper cleanup and absence of fabricated completion.

The library remains types-only, serde-only and `forbid(unsafe_code)`. The
executable denies unsafe code except in the single approved private Windows
boundary, reviewed in [review 4](04-windows-boundary.md). That boundary composes
exact handle inheritance, Job membership and optional ConPTY at process creation.

Madrillo CI uses public runners and ordinary processes. Pichi's actual CLI tests
its caller lifetime, terminal behavior, state reconciliation and direct guest-first
CBOR build-channel contract. No Dillo VM is executed by this conformance workflow;
Pichi/Dillo repository VM workflows remain separate. See [review 6](06-public-ci.md).

Compatibility boundaries remain explicit: macOS cleanup covers descendants in
the managed process group; workloads own deliberate escapes. Linux requires the
native pidfd/wait facilities documented in README. Systemd/cgroup facilities are
Linux-specific and explicit requests fail on unsupported hosts. Native Windows
uses MSVC, Jobs and ConPTY; the approved private unsafe boundary is not part of
the shared library. Guest command execution and OCI runtime policy belong to
callers and workloads. Monitor loss permits cooperative pipe-workload cleanup;
missing completion stays unknown, and an exit callback is never inferred from
a retained workload result. Filesystem durability limits and passthrough I/O
ownership remain documented in [README](../../README.md).

Pichi [PR #154](https://github.com/pichi-vm/pichi/pull/154) contains the caller
integration and retains its qualified Dillo dependency `88d507c`. The original
Dillo [PR #92](https://github.com/pichi-vm/dillo/pull/92) has since been closed
in favor of [#101](https://github.com/pichi-vm/dillo/pull/101) (input),
[#102](https://github.com/pichi-vm/dillo/pull/102) (resize), and
[#103](https://github.com/pichi-vm/dillo/pull/103) (input-owner lifetime).
Those replacement PRs have their own CI and are not the source pins validated
by this Madrillo run. At handoff, their Windows boot suite reports a missing
early-input acknowledgement in `console_input_reaches_guest`; console-loss and
resize tests pass in the [same job](https://github.com/pichi-vm/dillo/actions/runs/42238940631/job/127963842967).
No VM workflow was dispatched to investigate or retry that separate result.

Pichi's own [CI run](https://github.com/pichi-vm/pichi/actions/runs/42231906123)
subsequently finished with two failing jobs. The [macOS VM job](https://github.com/pichi-vm/pichi/actions/runs/42231906123/job/127952912747)
reports `monitor disconnected without completion` in all three build suites.
Its retained artifact contains the client error but omits `monitor.err`, so the
cause is unresolved. This may be an integration defect; the ordinary-process
qualification is not evidence that the VM path passes. The [Linux ARM64 job](https://github.com/pichi-vm/pichi/actions/runs/42231906123/job/127952912642)
failed when the native linker segfaulted while linking `staged_images`.
Windows and Linux x86_64 passed. PR #154 is ready for review, but its separate CI
is not fully green. These logs were inspected read-only; no VM run was dispatched
or retried, and the qualified sources were not changed speculatively.

Dillo's console-loss shutdown remains its default, and helpers follow the
approved process-group contract. The three sequential plan reviews and twelve
earlier simplification passes remain recorded with their historical scope
clearly identified. Integration review and merge decisions remain separate.
