# Final coverage review

This review establishes executable coverage, not a conformance pass. Native
reports must still establish all thirty requirements three consecutive times
on unchanged Madrillo, Pichi and Dillo sources.

The approved Windows boundary removes both implementation gaps: opaque handles
retain their numeric identities through native creation, and the owned Job
provides actual empty-scope observation for pipe and ConPTY workloads. All three
native monitor qualification jobs pass at `2a7993d`. The separately invoked
Windows caller fixture exposed an obsolete incomplete-cleanup expectation;
`6b9065e` removes that exception and requires complete cleanup for every case.

Pichi `cc46a598` supplies the matching first hop and refreshes its host packaging
patch. The shared run/build launcher passes the selected resource values and
unchanged Dillo arguments to Madrillo. The private Windows module supplies the
actual stdio handles and selected resources to one explicit native list. The
caller has no workload Job, proxy or guest protocol. The new caller test verifies
binary transport, resource exclusion and EOF through both hops. Denied breakaway,
caller Job loss and actual console loss retain their existing explicit fixtures.

C26 maps native caller tests for launch, attach, input, resize, detach, detached
startup, wait, stop, discovery, image/storage pins and crash reconciliation, plus
real Dillo and Pichi VM suites. C27 maps all three real build/boot suites on every
host. Those suites execute the existing guest-first CBOR exchange and inherited
named-port endpoints; no alternate transport or synthetic build replaces them.
The current Windows and macOS VM results remain unverified until those mapped
cases execute successfully. The complete mappings now permit that execution.

C09 explicitly requires the Unix job-control and Windows foreground-interrupt
cases in addition to size/resize/reattachment and drain tests. C10 explicitly
requires Unix exact-descriptor and Windows exact-handle evidence. C07/C18's
descendant, concurrent-spawn, force and timeout fixtures require complete cleanup
on every host under its documented scope contract. No ConPTY cleanup waiver
remains. The approved macOS process-group boundary remains unchanged.

The full runner checks clean source identity before and after every round,
requires native MSVC/Darwin/Linux toolchains, verifies exact command logs and
positive execution markers, and aggregates one report per host. Failures reset
the streak; neither an unexecuted case nor a successful exit without its markers
can count. Three fresh rounds are mandatory. Existing focused qualification
and earlier source revisions do not contribute to that streak.
