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
C01–C30 coverage remains pending and the full runner must fail closed.
