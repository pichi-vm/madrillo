# Final implementation and conformance review

All C01–C30 requirements passed three consecutive complete native rounds in
[run 42254131097](https://github.com/pichi-vm/madrillo/actions/runs/42254131097).
The executed Madrillo revision is `722f8b9e9b265f0318743141b74a16ab797cb1c8`,
with Pichi `c54f003d727c9b418db3422f49b0d7ca583fba1f`. This completion record
is a documentation-only follow-up to those exact sources.

| Full round | Linux | macOS | Windows MSVC | Verified streak |
| --- | --- | --- | --- | --- |
| `42254131097-1-1` | 30 cases passed | 18 cases passed | 14 cases passed | 1 |
| `42254131097-1-2` | 30 cases passed | 18 cases passed | 14 cases passed | 2 |
| `42254131097-1-3` | 30 cases passed | 18 cases passed | 14 cases passed | 3 |

Artifact `13521879814`, named `conformance-evidence`, contains the verified
ledger, all nine reports, exact command logs, native distributions and pinned
conmon oracle artifacts. The independent audit checked every one of the 186
command logs against its hash, command and required positive execution markers.
Source revisions and trees, native compiler platforms, nine unique invocation
IDs and all ledger entries matched. All nine distribution checksums, native
executable headers, exact bare-library sources, source-package revision records
and dependency license inventories were verified. Each Linux/Windows package
contains 73 dependency license entries; each macOS package contains 66.
The complete mapping remains in [CONFORMANCE.json](../../CONFORMANCE.json).

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

The final caller includes bounded retained startup errors and native `TMPDIR`
socket placement, with later clients using the retained address even when their
environment differs. It also preserves main's explicit image device-bus contract
without a second fallback planner. Native checks cover storage ownership and
exact inherited build-port values alongside caller lifetime and terminal use.
Both output streams are retained in evidence without interleaving test-result
lines; no prior failed report or missing marker was waived.

The library remains types-only, serde-only and `forbid(unsafe_code)`. The
executable denies unsafe code except in its single approved private Windows
boundary, reviewed in [review 4](04-windows-boundary.md). That boundary composes
exact handle inheritance, Job membership and optional ConPTY at process creation.
Madrillo's production implementation remains unchanged from `0e860df8`.

CI uses public runners and ordinary processes. Pichi's actual CLI verifies
caller lifetime, terminal behavior, state reconciliation and the direct
guest-first CBOR build-channel contract. No Dillo VM is built or executed by
this conformance workflow. See [review 6](06-public-ci.md).

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

Pichi [PR #154](https://github.com/pichi-vm/pichi/pull/154) is ready and mergeable
at the qualified revision. Its separate
[automatic CI](https://github.com/pichi-vm/pichi/actions/runs/42254103699) is not
fully green: static checks, public ARM64 workspace tests, packaging and the
x86_64 appliance build passed; ARM64 appliance execution remains queued and
the public macOS workspace job was canceled. Dependent native VM jobs have not
yet verified this final caller. Native ordinary-process conformance does not
stand in for those VM results.

The previous [macOS VM job](https://github.com/pichi-vm/pichi/actions/runs/42242041188/job/127994038063)
reported `Operation not permitted` at monitor startup. Pichi now honors the
runner's native temporary directory, addressing a suspected sandbox mismatch;
that explanation still needs confirmation on the affected runner. The previous
[ARM64 job](https://github.com/pichi-vm/pichi/actions/runs/42242041188/job/127994037922)
hit a nightly Rust compiler error in `nix` while building Madrillo. Pichi's
build helper now uses Madrillo's pinned toolchain; its real build and cleanup
passed locally. No VM workflow was manually dispatched or retried.

Pichi retains Dillo `88d507c`, with resize and default console-loss shutdown.
Stdin delivery [#101](https://github.com/pichi-vm/dillo/pull/101) has landed on
Dillo main. Resize is [#105](https://github.com/pichi-vm/dillo/pull/105); shutdown
[#103](https://github.com/pichi-vm/dillo/pull/103) merged into the older resize
branch, not main. The complete integration pin remains necessary until both
features land. The approved process-group contract for Dillo helpers is unchanged.

The three sequential plan reviews and twelve earlier simplification passes
remain recorded with their historical scope identified. Local final reports,
logs, ledger and requirement audit are retained under `conformance-runs`;
the earlier gate is separated under `archive/42232080329`. Temporary task files
were removed. Integration review and merge decisions remain separate.
