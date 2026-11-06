# Draft upstream API requests

These messages have not been posted. They describe the remaining dependency
capabilities needed by Madrillo's C07/C09/C10/C27 requirements without adding
owned unsafe code or interpreting a workload's arguments.

## WezTerm / filedescriptor: consuming File conversion preserving HANDLE identity

Target: <https://github.com/wezterm/wezterm/issues/new>

Title: `filedescriptor: consuming conversion to File without duplicating the Windows handle`

Madrillo is a generic Rust process monitor with `forbid(unsafe_code)`. Its caller
passes an inherited pipe HANDLE and an opaque workload argument containing that
same numeric HANDLE. The monitor must pass the handle to the workload unchanged;
it cannot interpret or rewrite the workload's arguments.

In filedescriptor 0.8.3, `FileDescriptor::new` owns an inherited handle, but
`as_file()` duplicates it. The duplicate is useful for ordinary I/O, but changes
the number and prevents composition with safe spawn APIs taking `AsHandle`.
The existing `IntoRawHandle` conversion requires an unsafe conversion back to
`File` or `OwnedHandle` in the consumer.

Would a consuming `try_into_file(self) -> Result<File>` (or Windows owned-handle
equivalent) fit the library? It should validate that the resource can be used as
a File, transfer ownership without duplication, preserve its numeric value, and
avoid double-close on error. An identity-preserving `AsHandle` implementation on
an appropriate validated handle type would also address the borrowing use case.

Acceptance fixture: adopt a real inherited pipe, obtain an owned/borrowed standard
handle through safe Rust, assert the numeric value is unchanged, transfer it via
a native explicit handle list, and verify binary I/O and exact EOF ownership.
Invalid values and sockets must not silently become valid File handles.

Reference: <https://docs.rs/filedescriptor/0.8.3/filedescriptor/struct.FileDescriptor.html>

## windows-spawn: explicit inheritance preserving existing HANDLE values

Target: <https://github.com/P4suta/windows-spawn/issues/new>

Title: `Support exact inherited HANDLE values for opaque child protocols`

Madrillo is a generic process monitor that forwards caller-owned resources to
an arbitrary workload without interpreting its argv or environment. A workload
may already have a numeric pipe HANDLE embedded in a private argument format.

windows-spawn 0.1.0 safely duplicates handles and lowers argument-handle tokens.
That works when the launcher constructs the arguments using those tokens. Here
the arguments are opaque, and an additional duplicate changes their meaning.

Could the API also accept an explicit list of borrowed, already-inheritable
handles whose numeric values must remain unchanged in the child? Rejecting
non-inheritable handles would be sufficient; changing their flags is unnecessary
for this caller contract. The list must compose with native stdio, an assigned
Job, ConPTY, native UTF-16 arguments/environment/cwd, and the existing suspended
or atomic creation transaction. Unlisted inheritable handles must stay excluded.

Acceptance fixture: pass a pipe HANDLE already embedded in an opaque argument,
spawn without rewriting that argument, verify the same number and binary I/O,
exclude an unrelated inheritable writer, and observe EOF when the actual owners
close. Exercise both ordinary pipe and ConPTY launch, including spawn failure.

Reference: <https://docs.rs/windows-spawn/0.1.0/windows_spawn/>

## conpty-oxide: Job completion and opaque resource inheritance

Target: <https://github.com/P4suta/conpty-oxide/issues/new>

Title: `Expose safe Job-empty observation and compose opaque inherited handles`

Madrillo uses conpty-oxide 0.1.2 for native Windows terminal workloads. It retains
the leader's full exit status separately from descendant-cleanup status. After
`Child::kill()`, waiting for the leader does not prove that its Job is empty, and
console-output EOF is not proof either: a descendant may have closed its console.

Could Child expose a safe, nonblocking Job-empty observation, or a borrowed Job
handle suitable for a safe accounting query? The owned Job identity must remain
alive until observation completes. Errors should remain distinct from an empty
Job; no PID-list snapshot should substitute for Job membership.

The monitor also needs to pass an explicit inherited HANDLE list without changing
the numeric values already embedded in opaque workload arguments. Could the
public Command/Pty API compose this with its existing windows-spawn transaction?
This would keep pseudoconsole lifetime management inside conpty-oxide, without
requiring consumer code to implement the unsafe `AsPseudoConsole` trait.

Acceptance fixtures: grandchildren and concurrent forks, descendants with closed
stdio, forced termination followed by observed Job emptiness, retained leader
exit bits, and an extra binary pipe passed unchanged through ConPTY creation.
Madrillo's Dillo workload uses ordinary pipes for cooperative console-loss
shutdown; this request does not require changing ConPTY's documented drop policy.

Reference: <https://docs.rs/conpty-oxide/0.1.2/conpty_oxide/blocking/struct.Child.html>
