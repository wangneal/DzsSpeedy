# Fix Asura Injection with One Hook-Based Lifecycle

## Problem Statement

On a 64-bit Windows system, DzsSpeedy correctly identifies Asura as a 32-bit/WOW64 target and selects the 32-bit bridge and `speedpatch32.dll`. Injection still fails because the current single-path injector starts a remote thread at `LoadLibraryW`; inside Asura that thread completes with exit code `0xffffffff`, and the DLL never appears in the target module list.

This is not an architecture mismatch, a missing packaged binary, or a Windows process-mitigation/signing policy failure. The installed bridge and DLL are both valid PE32 binaries, the DLL imports only Windows system libraries, and Asura does not enable Microsoft-only signatures, dynamic-code blocking, remote-image blocking, or extension-point blocking. The selected remote `LoadLibraryW` primitive is incompatible with this target.

Historical runtime evidence shows that the same class of 32-bit target was successfully injected through a thread-specific `SetWindowsHookExW` path and then remained `ENABLED`. The regression was introduced when that proven path was removed in favor of remote `LoadLibraryW` only.

## Solution

Use one injection mechanism for supported game targets: a same-architecture, thread-specific Windows message hook. The matching bridge tries the target's visible-window threads, hidden-window threads, and remaining process threads in that order, installs the same `WH_GETMESSAGE` hook on viable candidates, wakes them, and lets the first delivered callback initialize the DLL from an existing target thread instead of calling the target's `LoadLibraryW` from a newly created remote thread.

The hook callback must acquire exactly one explicit self-reference to the DLL, initialize speed control once outside loader lock, publish a structured result, and then allow the bridge to remove the hook immediately. This keeps the DLL resident without retaining hook handles or forcing the bridge to stay alive. Runtime ejection means logical disablement: the DLL and its hooks remain resident but inert until the target process exits.

There must be no fallback chain involving remote `LoadLibraryW`, `LoadLibraryA`, `LdrLoadDll`, manual mapping, or multiple injection methods. Unsupported targets without an eligible GUI message thread must fail with a precise error.

## User Stories

1. As a user on 64-bit Windows, I want DzsSpeedy to inject into a 32-bit Asura process, so that the operating-system bitness does not cause a false architecture failure.
2. As an Asura player, I want injection to use the target execution context already proven to work, so that the DLL is not rejected by Asura's remote `LoadLibraryW` behavior.
3. As a user, I want one deterministic injection method, so that failures identify one concrete stage instead of hiding behind a fallback chain.
4. As a user, I want the UI to report whether no GUI thread was found, hook installation failed, the hook was not invoked, DLL self-retention failed, initialization failed, or status confirmation timed out, so that failures are actionable.
5. As a user, I want a successful injection to reach `ENABLED`, so that a loaded-but-uninitialized DLL is never reported as success.
6. As a user, I want repeated status polling to remain `ENABLED` after the injection hook is removed, so that the DLL lifetime does not depend on the bridge retaining a hook handle.
7. As a user, I want closing DzsSpeedy to leave Asura responsive, so that bridge shutdown cannot freeze the game.
8. As a user, I want ejection to disable acceleration without unloading executable hook code from a running game, so that the target cannot crash on a live `FreeLibrary`.
9. As a user, I want failed initialization to produce the exact speedpatch initialization code, so that MinHook and shared-state failures are distinguishable from injection failures.
10. As a user, I want installation paths containing spaces and non-ASCII characters to work, so that the default Program Files installation remains supported.
11. As a user, I want repeated inject, status, disable, enable, and eject cycles to remain stable, so that normal use does not progressively destabilize the game.
12. As a maintainer, I want the bridge to reject bridge/target architecture mismatches before installing a hook, so that the selected DLL always matches the target process.
13. As a maintainer, I want every hook handle and local module reference to have explicit ownership, so that shutdown behavior is deterministic.
14. As a maintainer, I want target-exit races to be treated as target termination rather than generic injection failure, so that diagnostics reflect what happened.
15. As a maintainer, I want an automated fixture that reproduces `LoadLibraryW` returning `0xffffffff` on a remote-created thread, so that this regression cannot return.

## Implementation Decisions

- The bridge and target must have the same architecture. A 32-bit/WOW64 target uses the 32-bit bridge and DLL even on a 64-bit operating system.
- The only supported injection primitive is a thread-specific `SetWindowsHookExW` hook installed by the same-architecture bridge.
- Candidate selection must prefer visible top-level window threads, then hidden window threads, then remaining target threads. The bridge may install the same `WH_GETMESSAGE` hook on multiple viable candidates in one operation, but diagnostics must identify all candidates and the thread that actually ran the callback. Other injection primitives must not be attempted when selection or installation fails.
- The speedpatch DLL must export a minimal hook callback for both architectures. The callback must be reentrancy-safe and perform initialization only once.
- The callback executes outside `DllMain`. `DllMain` remains limited to loader-safe bookkeeping.
- On first callback execution, the DLL acquires one explicit module reference from its own callback address before reporting successful initialization. This reference replaces the old behavior of retaining hook handles to keep the DLL loaded.
- The bridge removes the hook after receiving a terminal initialization result. It must also remove the hook on timeout, target exit, and shutdown.
- Before installing hooks, the bridge creates and resets a per-process `DzsSpeedyHookComplete.<pid>` kernel event. `SP_HookProc` opens that event before acquiring its DLL self-reference and signals it only after publishing its terminal result. The event is the reliable callback-completion signal; UTF-16 DLL logs remain diagnostic-only and never control ownership or shutdown.
- Initialization publishes structured stages and error codes through the existing per-process status contract or an equivalent single shared result contract. At minimum, it distinguishes hook installation, callback invocation, module retention, speedpatch initialization, enabled, failed, and target exited.
- The bridge reports a command as successful only after the target publishes `ENABLED`. A hook handle alone is never success.
- Runtime ejection only publishes `DISABLED`. It must not remove MinHook detours, destroy the status mapping, release the explicit self-reference, or call remote `FreeLibrary`; Windows reclaims the resident DLL when the target exits.
- Bridge shutdown drains or cancels pending hook operations, removes all owned hook handles, and never leaves hook ownership as a reason for the bridge to remain resident.
- Injection, enable, disable, and logical-eject operations acquire a packed atomic admission lease. Shutdown atomically closes admission before disabling targets, waits for every existing lease, and performs a final disable pass. Shared-state writes use compare-and-swap so `FAILED` cannot be overwritten by a concurrent disable.
- Remote `LoadLibraryW`, `LoadLibraryA`, `LdrLoadDll`, manual mapping, and method fallback loops are explicitly excluded.
- Frontend errors preserve the complete bridge stage and native error instead of replacing them with a generic administrator message.

## Testing Decisions

- The primary test seam is the bridge command boundary because it covers architecture selection, hook installation, DLL initialization, status publication, and cleanup as one externally observable behavior.
- Add a same-architecture x86 GUI fixture with a real message loop. The fixture must make `LoadLibraryW` return `0xffffffff` when invoked from a foreign-created thread while remaining loadable through normal Windows hook delivery. This is the deterministic red-capable reproduction of the reported Asura failure.
- The regression test injects into that fixture, requires `INJECT` success and `STATUS ENABLED`, removes the injection hook, polls status again, ejects logically, requires `STATUS DISABLED`, and verifies that the fixture remains alive and responsive.
- Run the equivalent lifecycle fixture for x64 to prevent architecture-specific ownership regressions even though the reported target is x86/WOW64.
- Add failure tests for no eligible GUI thread, hook installation denial, callback timeout, initialization error, target exit during injection, and bridge shutdown during injection.
- Add a repeated lifecycle test covering at least 50 inject/status/disable/enable/logical-eject cycles and checking for leaked injection-hook handles, retained bridge processes, unexpected target exits, and target hangs.
- Validate a packaged release from a Program Files path on Windows 10 x64 against the 32-bit Asura target. Windows 11 x64 remains in the compatibility matrix.
- Existing bridge protocol tests remain the prior art for command/result assertions; existing real x86/x64 injection tests remain the prior art for lifecycle verification.
- Tests assert external states, error stages, process liveness, and cleanup. They do not assert internal helper calls.

## Out of Scope

- Supporting processes where no target thread accepts and dispatches a `WH_GETMESSAGE` hook.
- Bypassing kernel anti-cheat, protected-process-light, code-integrity enforcement, or third-party security products.
- Driver-based injection, manual PE mapping, or undocumented loader bypasses.
- Reintroducing a fallback chain of injection methods.
- Redesigning the speed-control hook set or changing speed semantics.

## Further Notes

- Reported target: Asura, PID 10964.
- Host: 64-bit Windows; target path selection and PE inspection confirm a 32-bit/WOW64 injection chain.
- Current result: the remote `LoadLibraryW` thread returns `0xffffffff`; `speedpatch32.dll` is absent from the module snapshot after one second of confirmation polling.
- Process mitigation inspection shows dynamic-code blocking, Microsoft-only signing, remote-image blocking, extension-point blocking, CFG, and user shadow stack are disabled. SEHOP is enabled and is not a DLL-loading prohibition.
- Historical log evidence records a successful `SetWindowsHookExW` injection into PID 35284 followed by sustained `ENABLED` status and working speed changes. This is the strongest available differential evidence for the selected solution.
- The live Asura PID 10964 test had 68 ordered thread candidates, accepted hooks on 17 threads, and initialized on callback thread 47196. This proves the process-thread candidate tier is required when Asura has no visible top-level window.
- A live Asura test completed `SP_Shutdown`, removed MinHook detours and the status mapping, and returned success from remote `FreeLibrary`; Asura exited immediately afterward. No crash dump was produced, so the precise internal fault is unavailable, but live physical unload is excluded from the supported lifecycle because it is the only changed operation in the observed failure.
- A follow-up live test against Asura PID 33040 completed `INJECT -> ENABLED -> DISABLE -> DISABLED -> ENABLE -> ENABLED -> EJECT -> DISABLED`, then shut down the bridge. Asura remained alive and responsive throughout and after bridge shutdown. The successful `EJECT` log contains only logical disablement and no remote unload operation.
- The latest-build live test against restarted Asura PID 43688 completed 50 consecutive `EJECT -> DISABLED -> INJECT -> ENABLED` cycles. Every command and status assertion passed, the final state was `DISABLED`, `bridge32.exe` exited with code 0 after its shutdown event, and Asura remained alive and responsive. Native evidence records `MH_Initialize=MH_OK`, all speed hooks installed, and `SP_HookProc` completing with result `0x00000000`; bridge shutdown records logical disablement only and no DLL unload.
- After the shutdown-admission, callback-monitoring, hook-ownership, persisted-error, and pipe-framing fixes, the final live test against Asura PID 10836 repeated the same 50-cycle sequence with no failures. The final state was `DISABLED`, bridge exit code was 0, Asura remained responsive, and the fresh native log again recorded `MH_Initialize=MH_OK`, all hooks installed, and `SP_HookProc` result `0x00000000`.
- The final Release build, including the kernel completion event protocol, completed 50 more cycles against Asura PID 15984. Every transition passed, bridge shutdown completed with exit code 0, the target remained responsive, and native logs contained no completion-event error. This is the release-binary acceptance result for the implemented protocol.
- After the final event-order and cross-integrity completion-event corrections, the rebuilt Release bridge completed 50 consecutive `EJECT -> DISABLED -> INJECT -> ENABLED` cycles against restarted Asura PID 15880. Every transition passed, the final state was `DISABLED`, `bridge32.exe` exited with code 0 after shutdown, Asura remained responsive, and the native log recorded `MH_Initialize=MH_OK`, all speed hooks installed, and `SP_HookProc` result `0x00000000`. This is the definitive final live acceptance run.
- The bridge unit suite covers candidate ordering, handshake publication ordering, shutdown admission, target exit, persisted failure decoding, and native error decoding. The administrator-only Asura lifecycle remains an explicit live regression harness under `target/codex-test`; a distributable x86/x64 GUI fixture is still separate test-infrastructure work and is not represented as automated CI coverage.
- The exact product-level root cause is the choice of an incompatible remote loader entry for Asura, not lack of administrator rights or host/target bitness mismatch. Whether Asura patches `LoadLibraryW` directly or rejects that call through another user-mode component can be captured as diagnostics, but it does not change the required single-path behavior.
- Bridge shutdown robustness (bounded drain, shutdown-aware pending-injection monitor, no health masquerade during shutdown, singleton takeover, GUI bridge repair) is specified separately in `fix-stale-bridge-shutdown-race.md`; its harness reproduces the reported `bridge shutdown is in progress` failure deterministically.
