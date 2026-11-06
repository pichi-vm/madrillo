# Full conformance runner

Qualification CI is useful evidence and does not count toward the three-pass
stopping rule. `tools/conformance.py` is the full-conformance gate. The current
manifest deliberately remains pending, so this command records failure before
executing any suites:

```
python3 tools/conformance.py run --round round-1 --report conformance-runs/round-1/linux.json
```

Each reviewed C01–C30 requirement must have `status: ready`, explicit native
hosts, and executable test mappings. `ready` means coverage is ready to run;
it does not mean the requirement passed. Test cases require positive output
markers as well as exit zero, so a filter selecting zero tests is not success.
The current case mappings are partial and must be completed before promotion.

Run the same round on Linux, macOS, and Windows MSVC with unchanged, clean
sources. Collect each JSON report together with its sibling log artifacts.
The aggregator checks every required native case, source revision/tree, round
identity, actual exit status, output markers and artifact hashes:

```
python3 tools/conformance.py aggregate --round round-1 --ledger conformance-runs/ledger.json conformance-runs/round-1/linux.json conformance-runs/round-1/macos.json conformance-runs/round-1/windows.json
```

Aggregate failed and missing-native rounds too; they reset the streak. Reusing a
round or native invocation cannot count again. Changes to source identity start
a new streak. Three fresh successful aggregates on the same source identity are
required. Reports and the ledger remain outside tracked implementation sources.
The manifest's initial `clean_streak` is not accepted as evidence by the runner.

Remaining harness work includes full failure-injection and native case mappings,
pinned conmon build/differential artifacts, pichi/dillo source identity and
integration evidence, and native CI round orchestration. Until those are present,
the full runner must fail closed while any requirement remains pending.

## Reviewed native launch and reconnection coverage

C02 is ready to execute on all three native hosts. The required positive markers
cover literal shell metacharacters, empty arguments, trailing backslashes,
embedded quotes, native non-Unicode argv/environment, working-directory
selection, and retained spawn failure. Both pipe and terminal modes run the
native context fixture. APFS rejects invalid filename bytes itself; that case
uses a Unicode working directory while still verifying raw argv/environment.
The source uses native process APIs and retains a typed `SpawnFailed` outcome.

C05 is ready to execute on all three native hosts. Required markers cover an
inherited initial connection receiving immediate first output without sending
a handshake, endpoint reattachment after writer disconnect, terminal reconnect,
and a draining viewer receiving a large startup burst. The disconnect fixture
checks that a new attachment receives only newly emitted bytes. Source review
confirms attachment queues contain pending delivery only: no historical replay
buffer or sequence protocol is present.

These mappings add required positive markers to the existing native test suite;
they do not rerun the suite for each requirement. Readiness is a coverage review,
not a conformance pass. Caller terminal/Job loss remains separately tracked by
C04, and Windows terminal descendant cleanup remains pending under C07/C09.
