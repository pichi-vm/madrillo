# Public-runner conformance review

On November 6 the user directed Madrillo CI to use public runners and no Dillo
VM tests. They clarified that this applies to Madrillo CI; Pichi/Dillo's existing
workflows stay unchanged and must not be manually dispatched for this task.

The native matrix now uses Ubuntu, macOS and Windows hosted runners. The Dillo
qualification workflow and VM orchestration scripts are removed. Conformance
does not check out Dillo, install guest toolchains, reserve hugepages or require
KVM/HVF/WHP. Linux's cgroup-v1 fixture retains its disposable QEMU kernel with
CPU-emulation fallback; it does not execute Dillo.

C24 now names the generic monitor contract. An ordinary workload deliberately
leaves stdin unread while writing console output. Its input is tested both empty
and backpressured. Killing the monitor closes the console; the workload then
closes its own helper's input, waits for that helper's clean exit, and records
its completed cleanup. This proves the monitor's pipe/Job lifetime does not
preempt cooperative shutdown. Dillo's own handling of input hangup remains a
workload contract, not a feature or dependency of Madrillo.

C26 uses actual Pichi caller/CLI fixtures for launch, attachment, resize,
detach, stop, retained state and reconciliation. C27 adds an ordinary native
child to the real `pichi build` launch path on every host. The child requires an
empty response pipe before sending the serialized build request, checks the
exact response bytes and records the original named-port/mount arguments. It
exits 37 deliberately: the test checks channel preservation and exit propagation
without pretending that a guest build ran or generating a substitute VM image.

The three-round gate still requires all C01–C30 mappings, every selected native
case, positive execution markers, hashed logs and unchanged source identities.
Removing VM dependencies does not waive process scope, terminal, logging,
timeout, service, durability or packaging tests. New native fixture execution
and three fresh complete rounds remain required; no prior partial run counts.
