/* nopump.c — regression fixture for the "injection spins forever" bug.
 *
 * Mirrors the Asura injection outcome observed in the field:
 *   - threads DO have message queues (so SetWindowsHookExW accepts hooks),
 *   - but NO thread ever pumps messages again (no GetMessageW/PeekMessageW
 *     after startup), so the WH_GETMESSAGE hook callback can never fire and
 *     the hook DLL is never injected into this process.
 *
 * This is the exact state that left the old bridge's pending-injection
 * monitor in INITIALIZING forever (STATUS -> OK INITIALIZING endlessly).
 *
 * Build (zig bundles its own mingw libc for the gnu targets):
 *   zig cc -O2 -target x86_64-windows-gnu nopump.c -o nopump64.exe
 *   zig cc -O2 -target x86-windows-gnu     nopump.c -o nopump32.exe
 */
#include <windows.h>
#include <stdio.h>

static DWORD WINAPI idle_worker(LPVOID arg) {
    /* Create a real message queue on this thread, then never pump again. */
    MSG msg;
    PeekMessageW(&msg, NULL, 0, 0, PM_NOREMOVE);
    for (;;) {
        Sleep(2000);
    }
    return 0;
}

int main(void) {
    /* Main thread: also creates a queue, then sleeps forever. */
    MSG msg;
    PeekMessageW(&msg, NULL, 0, 0, PM_NOREMOVE);
    for (int i = 0; i < 4; i++) {
        CreateThread(NULL, 0, idle_worker, NULL, 0, NULL);
    }
    printf("NOPUMP_READY pid=%lu\n", GetCurrentProcessId());
    fflush(stdout);
    for (;;) {
        Sleep(5000);
    }
    return 0;
}
