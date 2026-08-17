/* probe32.c / probe64.c — inert hook-DLL probe.
 * Loaded into the target via SetWindowsHookExW; DllMain records the load
 * into %TEMP%\probe-loaded-<pid>.txt. Exports ProbeHookProc (WH_GETMESSAGE
 * pass-through) so the driver can reference it. Does NOTHING else — no
 * acceleration, no timers. */
#include <windows.h>
#include <stdio.h>

__declspec(dllexport) LRESULT CALLBACK ProbeHookProc(int nCode, WPARAM wParam, LPARAM lParam) {
    return CallNextHookEx(NULL, nCode, wParam, lParam);
}

BOOL WINAPI DllMain(HINSTANCE hinst, DWORD reason, LPVOID reserved) {
    if (reason == DLL_PROCESS_ATTACH) {
        char path[MAX_PATH];
        char buf[256];
        DWORD n = GetTempPathA(sizeof(path), path);
        if (n > 0 && n < sizeof(path)) {
            snprintf(buf, sizeof(buf), "%sprobe-loaded-%lu.txt", path, GetCurrentProcessId());
            FILE *f = fopen(buf, "w");
            if (f) {
                fprintf(f, "loaded pid=%lu tid=%lu reason=attach\n",
                        GetCurrentProcessId(), GetCurrentThreadId());
                fclose(f);
            }
        }
    }
    return TRUE;
}
