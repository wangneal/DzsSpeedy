/* hooktest hook DLL: WH_GETMESSAGE hook proc that drops marker files.
 * Marker presence proves (a) DLL loaded into target (dllmain) and
 * (b) hook callback actually delivered (hookproc).
 */
#include <windows.h>
#include <stdio.h>

static void write_marker(const char *which) {
    char tmp[MAX_PATH];
    if (!GetTempPathA(MAX_PATH, tmp)) return;
    char path[MAX_PATH];
    snprintf(path, sizeof(path), "%shooktest-%s-%lu.txt", tmp, which, GetCurrentProcessId());
    FILE *f = fopen(path, "w");
    if (f) {
        fprintf(f, "%s pid=%lu tid=%lu\n", which, GetCurrentProcessId(), GetCurrentThreadId());
        fclose(f);
    }
}

BOOL WINAPI DllMain(HINSTANCE hInstance, DWORD reason, LPVOID reserved) {
    (void)hInstance; (void)reserved;
    if (reason == DLL_PROCESS_ATTACH) {
        write_marker("dllmain");
    }
    return TRUE;
}

__declspec(dllexport) LRESULT CALLBACK SP_TestHookProc(int code, WPARAM wParam, LPARAM lParam) {
    if (code >= 0) {
        write_marker("hookproc");
    }
    return CallNextHookEx(NULL, code, wParam, lParam);
}
