# Fix Infinite INITIALIZING (Eternal UI Spinner) + Real Root Cause: Anti-Cheat Signature Gate

## Symptom

Clicking inject on an Asura (斗战神) process makes the UI spinner spin forever:
the process is neither accelerated nor reported as failed. Protocol-level
reproduction (live Asura PID 6464, current release bridge32):

```
INJECT 6464 -> ERROR INJECTION_PENDING: SetWindowsHookExW installed for pid=6464,
  hooked_threads=[11616, 14664, ... 13 threads], posted_threads=[all 13],
  but SP_HookProc did not publish its handshake within 15s (15190ms)
STATUS 6464 -> OK INITIALIZING   (for 120+ s; forever until bridge shutdown)
```

The same reproduction is deterministic without the game: a windowless fixture
whose threads own message queues but never pump again (`_tools/hooktest/nopump.c`,
`test-nopump.ps1`) yields `VERDICT=RED_SPIN` on the unfixed bridge.

## Root Cause Chain

1. **Target side (game)**: Asura accepts the `WH_GETMESSAGE` hook on 13 threads
   and consumes the posted `WM_NULL`, but the system's internal hook-DLL
   injection — the `LdrLoadDll` of the speedpatch DLL inside the target — is
   silently rejected. No error is surfaced by Windows: the callback is skipped,
   the DLL never appears in the target module list, and `DzsSpeedy.<pid>` never
   exists.

2. **The gate is an Authenticode trust check, not a hook-type/thread/driver
   check** (verified live against TerSafe.dll, the Tencent anti-cheat loaded in
   Asura, PID 14800):

   | Injection path | Unsigned DLL | Trusted-signed DLL |
   |---|---|---|
   | SetWindowsHookEx (all hook types, incl. WH_GETMESSAGE) | **blocked silently** (hook OK, DLL never loaded) | **allowed** (loaded, callbacks fire) |
   | CreateRemoteThread + LoadLibraryW | **blocked** (LoadLibraryW returns -1) | **blocked** (path itself refused) |

   Evidence: the bsjl1 (变速精灵免费版) `inproc.dll` — WoSign-signed in 2010,
   still `Valid` thanks to its timestamp — loads into Asura via WH_GETMESSAGE;
   the **same file with its signature stripped** is rejected; the same file
   renamed to `speedpatch32.dll` is still accepted (name/hash are irrelevant).
   A self-signed probe is rejected until its root is installed into the machine
   TRUSTED ROOT store — after which it is accepted. Conclusion: TerSafe runs a
   standard WinVerifyTrust trust-chain check on any DLL the game process is
   asked to load; unsigned/self-signed DLLs are dropped silently, while
   trusted-signed ones pass — which is why the 2010 signed bsjl1 keeps working
   and why the qmt/360-era reports of success exist (different protection
   states).

3. **Bridge (the bug)**: `monitor_pending_injection` had **no deadline** for the
   state "hooks installed (`hooks == Some`) but handshake never appears and
   completion never signals". Its only terminations were target exit, handshake
   completion, and bridge shutdown. In this state it looped forever while
   holding `InjectionStage::Initializing`, so `STATUS` kept answering
   `OK INITIALIZING` indefinitely.

4. **UI**: `INJECTION_PENDING` is treated as "keep waiting and poll STATUS"
   (`bridgeOutcomeNeedsStatus`), and the spinner is shown for
   `phase == "initializing"`. With STATUS never leaving INITIALIZING, the
   spinner can never resolve — "不成功也不失败".

## Fix

### A. Bridge fast-fail + deadline (`src-bridge/src/main.rs`, +90 lines)

1. **Fast-fail probe** (`DLL_INJECTION_PROBE_GRACE = 8s`, inside
   `wait_for_hook_callback`): after hooks are installed and `WM_NULL` posted,
   if no handshake/completion has appeared within 8s, probe the target module
   list with `find_remote_module`. If speedpatch is absent and the target is
   alive, return an explicit error naming the real failure ("the target
   rejects the hook-DLL injection (anti-cheat protection) or never dispatches
   WH_GETMESSAGE callbacks"), instead of the generic 15s timeout text.

2. **Monitor deadline** (`PENDING_INJECTION_DEADLINE = 30s`): the pending
   monitor now gives up when it has made no terminal progress within 30s —
   releases hook handles (best effort), records an explicit `Failed` detail
   via the existing publication path, and terminates (releasing its operation
   lease). `STATUS` then returns `FAILED <detail>`, so the UI shows a concrete
   error instead of the eternal spinner.

### B. Real fix — trusted code signing (`_tools/codesign/sign-speedpatch.ps1`)

The injection actually succeeds once the speedpatch DLL carries a trust-chain
valid Authenticode signature. No commercial certificate is required:

1. `sign-speedpatch.ps1` builds a self-signed root CA, installs its root into
   `LocalMachine\Root` (admin), issues a code-signing cert from it, and signs
   `speedpatch32.dll` / `speedpatch64.dll` (osslsigncode, SHA256).
2. Windows/WinVerifyTrust then treats the DLLs as validly signed; TerSafe
   allows the hook injection; the existing bridge mechanism (WH_GETMESSAGE +
   PostThreadMessageW wake) is unchanged and works end-to-end.
3. Deployment on other machines: install `dzsspeedy-root.cer` into their
   TRUSTED ROOT store (same script does it locally).

## Verification

- Red (pre-fix): `_tools/hooktest/test-nopump.ps1 x64 <bridge64.exe>` →
  `VERDICT=RED_SPIN`.
- Green (post-fix): same script against a rebuilt bridge →
  `VERDICT=GREEN_FAST_FAIL` (INJECT errors with the DLL-not-loaded message at
  ~8s) and `STATUS` reaches `FAILED` within the 30s monitor deadline.
- Live Asura, unsigned DLL: `INJECT <asura pid>` errors with the probe message
  at ~8s and `STATUS` becomes `FAILED` at ~38s instead of INITIALIZING forever.
- Live Asura, trusted-signed DLL (this release): `INJECT` succeeds — DLL loads,
  handshake completes, `STATUS` returns `OK ENABLED`, speed control works.
- Positive control after signing: fixture64 + bridge64 still `INJECT -> OK`
  at ~85ms (`_tools/hooktest/positive-control.ps1`).
- Probe harness used for the matrix above: `_tools/hooktest/wprobe/` (zig-built
  hook/remote-thread/module probes), results in `result-wprobe.txt`.

## Out of Scope

- Bypassing or nulling the anti-cheat check itself; the remote-thread
  injection path stays blocked by TerSafe regardless of signing.
- Driver-based injection (unsigned drivers cannot load on Win10/11 x64).
- Multi-method fallback in the bridge beyond the probe + deadline above.
