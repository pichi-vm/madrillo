# Plan review 2: native feasibility and lifecycle

Reviewed the revised plan against native job/cgroup/PTY boundaries and the
source audit's open qualification issues. Inspected launch-to-finalization
ordering, handle ownership, failure behavior, and each host's required evidence.
This is a second sequential self-review, not runtime conformance.

Findings corrected:

1. Worker isolation alone did not bound worker creation or explain blocked
   fsync. Added a fixed bounded worker model and explicit sync-failure semantics;
   timeout/control responsiveness must be tested under blocked sinks.
2. Handle duplication could leave original inherited writers open and mask
   monitor death. Added original-copy closure and inheritance assertions.
3. A safe PTY demo and safe Job Object demo do not prove safe combined spawn.
   Added a requirement to qualify them together before selecting dependencies.
4. Windows kill-on-close can contradict the required dillo cleanup behavior.
   Added an explicit ownership/drop-policy gate and root-crash requirement.
5. Passthrough cannot promise captured-mode I/O survival or reconnect. Scoped
   C04's I/O guarantee without weakening monitor lifetime or captured mode.
6. Exit-helper failure must not overwrite the leader's native result. Added
   separate engine cleanup outcomes and no exactly-once callback claim.

Open requirements retained: arbitrary descendant containment on macOS, safe
native resource adoption/placement/activation, and native runner access. A
process-group-only implementation cannot pass C07. Permission-denied Linux
features need explicit evidence and policy, not silent fallback to one PID.

Outcome: implementation can begin with these qualification gates. The review
does not certify feasibility of every native combination or allow omitted scope.
