# Fix Stale-Bridge Shutdown Race ("bridge shutdown is in progress")

## Problem Statement

On Windows 10 22H2, INJECT fails with `INJECT <pid>: bridge shutdown is in progress; operation was not started` and stays broken across app restarts. The bridge the GUI talks to is not a healthy freshly started bridge: it is a **stale bridge that is mid-shutdown** and refuses every new operation.

The failure is reported on Windows 10 22H2 only; Windows 11 Home/Pro are unaffected. The OS difference is a trigger-frequency difference (see Further Notes), not the mechanism — the mechanism is OS-independent and was reproduced deterministically in a harness on Windows 11.

### Root-cause chain (verified end to end)

1. `INJECT` installs `WH_GETMESSAGE` hooks and waits up to 15 s for `SP_HookProc` to publish a terminal handshake. When the callback is never delivered (frozen/busy target, threads that never pump messages), the injection goes **pending** and `monitor_pending_injection` takes over. That monitor is an **unbounded loop** that holds the bridge's `RemoteOperationLease` until the target publishes a terminal handshake or exits.
2. The user closes DzsSpeedy (app exit signals `Global\DzsSpeedyBridge64Shutdown` / `...32Shutdown`). The bridge sets its `OPERATION_SHUTDOWN_BIT`, disables tracked targets, and then waits `while active_remote_operations() != 0` — **unbounded**. With the pending-injection lease held forever (target alive, handshake never published), the bridge never exits: a zombie with the shutdown bit set that still owns the pipe singleton.
3. On restart the GUI spawns a fresh bridge, but the new bridge silently defers (`exit 0`) whenever *any* pipe owner answers a `GETSPEED` probe — including the zombie. The GUI binds to the zombie: health probes still answer `OK` (GETSPEED is not gated on the shutdown bit), so the UI shows a healthy bridge while every `INJECT`/`ENABLE`/`DISABLE` is refused with "bridge shutdown is in progress". The GUI never respawns a bridge (`ensure_bridges` runs once at startup and never prunes exited children), so the failure persists until the target process exits or the zombie is killed manually.

## Solution

### Bridge (src-bridge)

- **Bounded shutdown drain.** `shutdown_bridge` waits at most `SHUTDOWN_DRAIN_DEADLINE` (8 s) for operation leases, then exits anyway. In-flight completion paths already observe the shutdown bit and write the final `DISABLED`, so exiting early can never leave a target accelerated.
- **Shutdown-aware pending-injection monitor.** When shutdown is requested, the monitor gives the target `SHUTDOWN_PENDING_GRACE` (2 s) to publish its terminal handshake; on expiry it releases its hook handles, writes the final disable, clears the injection stage, and returns — releasing the lease. A stuck pending injection can no longer pin shutdown forever.
- **No masquerading health.** Once the shutdown bit is set, `GETSPEED`/`PING`/`VERSION` answer `ERROR bridge is shutting down` instead of `OK`, so GUI health checks and singleton-takeover probes can distinguish a dying bridge from a live one.
- **Singleton takeover.** A fresh bridge that finds the pipe owned by a non-healthy instance waits (bounded, 12 s) for the owner to leave and then takes over, instead of `exit 0`-ing into the zombie's shadow. Healthy owners still win immediately (`exit 0`).
- **Startup event reset.** The manual-reset `Global` shutdown event is reset at startup so a leftover signaled event can never shut down a freshly started bridge.
- **Singleton-owner deferral is verified by harness.** `acquire_bridge_singleton` intentionally leaks the `HANDLE` (Copy type, no Drop — binding to `_` keeps the kernel mutex owned for the process lifetime). A second bridge of the same arch must `exit 0` while the owner serves the pipe. The client machine's log (`debug/1.txt`) showed two bridge pairs alive at once (launched ~60 s apart, both answering `GETSPEED`); harness check D proves a healthy owner now wins and the duplicate defers.

### GUI (src-tauri)

- **Repair path with retry-once.** `bridge_inject`/`bridge_enable`/`bridge_disable` failures whose error reports shutdown ("shutdown is in progress" / "is shutting down") or a missing pipe trigger `repair_bridge(arch)`: wait (bounded, 12 s) for any stale owner to vanish or become healthy, then respawn exactly the missing arch's bridge, and retry the command once. The user's first click after a restart now succeeds instead of failing.
- **Liveness-aware bridge lifecycle.** `BRIDGE_CHILDREN` tracks the exe name per child, prunes exited children, and `ensure_bridges` respawns per-arch when no live child covers the pipe. `shutdown_bridges` waits up to 6 s for children (the bridge's own bounded grace).

## Testing Decisions

- **Deterministic end-to-end harness** (`target/codex-test/run-shutdown-race.ps1`): spawns the real `bridge64.exe` plus a same-architecture GUI fixture that accepts hooks but never pumps messages, so the injection goes pending and holds the lease; signals the real shutdown event; then asserts: (A) health probe is not faked during shutdown, (B) the bridge exits within 30 s, (C) a fresh bridge takes over the pipe. Red on the pre-fix binary (all three fail), green on the fixed binary.
- **Bridge unit tests**: `drain_respects_deadline_when_a_lease_is_held_forever`, `drain_returns_immediately_when_no_leases_are_held`, `shutdown_gating_covers_health_probes_only`, plus the existing admission tests.
- **GUI unit tests**: `detects_bridge_shutdown_responses_for_repair`.
- The user-reported symptom text (`ERROR bridge shutdown is in progress; operation was not started`) is preserved verbatim so frontend error surfacing and existing tests are unaffected.

## Further Notes

- The client's `debug/1.txt` log proves the installed binary was **older than the GitHub release**: its injection log strings (`try_windows_hook_x86`, `try_ldr_load_dll_x86`, `try_inject_impl`, "mapping not created within 5s", `LoadLibraryA success`) exist only in commits up to `9fe1cc4` (pre-v0.1.6) — the LoadLibrary-fallback era. The binary lived at `F:\bridge64.exe` / `F:\speedpatch32.dll` (a portable copy extracted to a drive root), while the machine later also ran an e62803f-era build (the shutdown-zombie session, `pending hook monitor` lines, target pid 8224). Multiple builds coexisting on the client is itself a deployment hazard: replace/remove stale portable copies before installing a fixed build, otherwise old zombie bridges keep answering the pipe.
- The client log also shows the hook-callback delivery on that Win10 machine is slow (>5 s, sometimes never within 15 s): the old 5 s mapping timeout plus LoadLibrary fallbacks produced confusing double failures (`gle=183` = DLL already loaded by the late hook callback). The current single-hook path waits 15 s and hands a pending injection to the monitor, which under shutdown now abandons after a 2 s grace — no more false "already loaded" cascades, no more zombie.
- The reported target is Asura (32-bit/WOW64 → bridge32). The harness runs both chains; the changes are arch-symmetric (x86 shares the same code).
- Why Windows 10 22H2 only: the error requires the shutdown bit to be set at INJECT time, which requires an app exit while a pending injection (or its 15 s wait) is in flight. On Win11 the hook callback publishes in time on the same targets, so injections complete before exit and no zombie forms. The Win10 bridge log (`%TEMP%\dzsspeedy-bridge.log`, timestamps are Unix seconds, format `[secs.millis] [pid=N] msg`) records `shutdown event received` / `shutdown grace expired` lines that pin the exact trigger on a given machine.
- The old build wording "RAM operation was not started" (vs current "operation was not started") indicates an installed binary older than the current source; users should update to a build containing this fix.
- The bridge's health response change (GETSPEED → `ERROR bridge is shutting down` during shutdown) is intentionally visible to the frontend: the B64/B32 chips go red during shutdown instead of green, which is the honest state.
