# Plan review 3: testability, order, and stopping rule

Reviewed all C01-C30 success criteria against milestone exits, native execution
requirements, the previous two reviews, and the user's three-clean-pass rule.
This is the third sequential self-review before goal activation.

Findings corrected:

1. Differential tests could accidentally use the installed conmon version
   rather than the pinned source. Added reference pin/build requirements and
   narrowly defined normalization so lost output cannot be normalized away.
2. A coverage manifest could become a checklist without evidence. Required
   test IDs/artifacts/source digests and runner validation; all entries start
   pending. Smoke checks cannot increment the conformance streak.
3. Repeating easy tests three times could hide races. Added fresh instances,
   race iterations, failure injection, and isolated resource-limit fixtures.
4. Verified full-pass scope: native Linux/macOS/Windows, all required functions,
   library/safety/package audits, and actual pichi/dillo integration. Neither
   cross-compilation nor an unavailable runner counts as a full clean pass.

Simplicity check: one package, bare shared types, private executable modules,
standard log/notification formats, one inherited initial channel and existing
reconnect/control channel; no added broker, plugin framework, replay service,
child API proxy, or second always-running supervisor.

Outcome: the implementation plan is reviewed and actionable. Native feasibility
gates remain explicit and are implementation work, not passed acceptance cases.
All three plan reviews are complete; the conformance streak remains zero.
