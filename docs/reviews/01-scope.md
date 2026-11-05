# Plan review 1: scope and precedent

Reviewed the initial plan against conmon's pinned cli.c option inventory,
ctr_exit.c signal handling, the source responsibility audit, and the user's
requirements. This was a self-review, not a delegated or runtime conformance pass.

Findings corrected:

1. Monitor receipt of SIGTERM/SIGQUIT/SIGINT was only implicit in milestone 6.
   Conmon forwards these while retaining supervision. Added C30 so normal
   monitor termination is not accidentally treated like a crash. Native Windows
   semantics must be explicit; terminal control bytes are a different path.
2. Diagnostics did not explicitly require native syslog, version/API identity,
   and help. Expanded C25; unavailable platform facilities remain explicit.
3. Corrected LISTEN_FDNAMES spelling in C17 to make the activation fixture precise.

Checked retained scope: all logging families, tty, descendants, initial-output
ordering, stdin retention, timeout, notify/activation, OOM, placement, held
resources, exit command, durable results, native hosts, and pichi/build boundary.
OCI runtime construction and guest execution remain justified exclusions.

Outcome: scope mapping is complete enough to proceed to feasibility review.
No implementation capability or clean conformance pass is claimed.
