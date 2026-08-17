/* Multi-hook-type probe: install several hook types on one target thread with the
 * same DLL, then watch for DLL load (DllMain marker) and callback (hookproc marker).
 * Usage: hookmulti <pid> <dllpath> <tid>
 * Hook types: WH_FOREGROUNDIDLE(11) + WH_GETMESSAGE(3); then WH_CBT(5) w/ activation nudge.
 */
#include <windows.h>
#include <stdio.h>
#include <tlhelp32.h>

static HHOOK hGm = NULL, hFg = NULL, hCbt = NULL;

static void status(const char *s) { printf("[%lu] %s\n", GetTickCount() / 1000, s); fflush(stdout); }

int main(int argc, char **argv) {
    if (argc < 4) { printf("usage: hookmulti <pid> <dllpath> <tid>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 10);
    const char *dll = argv[2];
    DWORD tid = (DWORD)strtoul(argv[3], NULL, 10);

    HMODULE hmod = LoadLibraryA(dll);
    if (!hmod) { printf("local load failed err=%lu\n", GetLastError()); return 1; }
    HOOKPROC proc = (HOOKPROC)GetProcAddress(hmod, "SP_TestHookProc");
    if (!proc) proc = (HOOKPROC)GetProcAddress(hmod, "SP_TestHookProc@12");
    if (!proc) { printf("no export\n"); return 1; }
    printf("dll=%s proc=%p tid=%lu\n", dll, proc, tid);

    char mp[MAX_PATH];
    snprintf(mp, sizeof(mp), "%s\\hooktest-dllmain-%lu.txt", getenv("TEMP"), pid);
    char hp[MAX_PATH];
    snprintf(hp, sizeof(hp), "%s\\hooktest-hookproc-%lu.txt", getenv("TEMP"), pid);
    DeleteFileA(mp);
    DeleteFileA(hp);

    int dll_seen = 0, proc_seen = 0;
    DWORD start = GetTickCount();

    /* 1) WH_FOREGROUNDIDLE — fires whenever the target thread goes idle-waiting */
    hFg = SetWindowsHookExW(WH_FOREGROUNDIDLE, proc, hmod, tid);
    printf("WH_FOREGROUNDIDLE: %s err=%lu\n", hFg ? "OK" : "FAIL", GetLastError());
    fflush(stdout);

    /* 2) WH_GETMESSAGE + post */
    hGm = SetWindowsHookExW(WH_GETMESSAGE, proc, hmod, tid);
    printf("WH_GETMESSAGE: %s err=%lu\n", hGm ? "OK" : "FAIL", GetLastError());
    PostThreadMessageW(tid, WM_NULL, 0, 0);
    printf("posted WM_NULL\n");
    fflush(stdout);

    /* 3) WH_CBT — fires on window events; we nudge with a harmless activation of a
     * message-only window we create on the same thread? CBT fires in our own thread for
     * our own window; to trigger in target we would need target window activity.
     * Instead rely on FGID+GETMESSAGE; report CBT install result only. */
    hCbt = SetWindowsHookExW(WH_CBT, proc, hmod, tid);
    printf("WH_CBT: %s err=%lu\n", hCbt ? "OK" : "FAIL", GetLastError());
    fflush(stdout);

    /* also try WH_SHELL? too invasive globally; skip. */

    while (GetTickCount() - start < 25000) {
        if (!dll_seen && GetFileAttributesA(mp) != INVALID_FILE_ATTRIBUTES) {
            dll_seen = 1;
            printf(">>> DLL LOADED (DllMain marker) at +%lus\n", (GetTickCount() - start) / 1000);
            fflush(stdout);
        }
        if (!proc_seen && GetFileAttributesA(hp) != INVALID_FILE_ATTRIBUTES) {
            proc_seen = 1;
            printf(">>> HOOKPROC FIRED at +%lus\n", (GetTickCount() - start) / 1000);
            fflush(stdout);
        }
        if (proc_seen) break;
        Sleep(250);
    }

    /* final module check */
    int in_target = 0;
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
    if (snap != INVALID_HANDLE_VALUE) {
        MODULEENTRY32W me; me.dwSize = sizeof(me);
        while (Module32NextW(snap, &me)) {
            if (_wcsicmp(me.szModule, L"hookdll32.dll") == 0 ||
                _wcsicmp(me.szModule, L"hookdll64.dll") == 0) { in_target = 1; break; }
        }
        CloseHandle(snap);
    }
    printf("dll_in_target=%d dll_seen=%d proc_seen=%d\n", in_target, dll_seen, proc_seen);

    if (hGm) UnhookWindowsHookEx(hGm);
    if (hFg) UnhookWindowsHookEx(hFg);
    if (hCbt) UnhookWindowsHookEx(hCbt);
    printf("DONE\n");
    return 0;
}