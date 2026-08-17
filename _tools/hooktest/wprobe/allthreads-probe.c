/* allthreads-probe.c — hook EVERY thread of the target with WH_GETMESSAGE
 * (like DzsSpeedy does, but ALL threads, not just window threads) and check
 * whether the probe DLL finally gets loaded. Answers: is the pump-thread
 * a non-window thread DzsSpeedy never hooked?
 * Usage: allthreads-probe <pid>
 */
#include <windows.h>
#include <tlhelp32.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

typedef struct { HHOOK h; DWORD tid; } HookRec;

static int dll_loaded_in_target(DWORD pid) {
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
    if (snap == INVALID_HANDLE_VALUE) return -1;
    MODULEENTRY32 me;
    me.dwSize = sizeof(me);
    int found = 0;
    if (Module32First(snap, &me)) {
        do {
            if (_stricmp(me.szModule, "dllprobe.dll") == 0) { found = 1; break; }
        } while (Module32Next(snap, &me));
    }
    CloseHandle(snap);
    return found;
}

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: allthreads-probe <pid>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 0);

    char dll_path[MAX_PATH];
    GetModuleFileNameA(NULL, dll_path, sizeof(dll_path));
    {
        char *slash = strrchr(dll_path, '\\');
        if (slash) strcpy(slash + 1, "dllprobe.dll");
    }
    HMODULE local = LoadLibraryA(dll_path);
    if (!local) { printf("LOCAL_LOAD=ERR:%lu\n", GetLastError()); return 1; }
    const char *names[] = { "ProbeHookProc@12", "ProbeHookProc", "_ProbeHookProc@12", "_ProbeHookProc" };
    HOOKPROC proc = NULL;
    for (int k = 0; k < 4 && !proc; k++) proc = (HOOKPROC)GetProcAddress(local, names[k]);
    if (!proc) { printf("PROC_ERR\n"); return 1; }

    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
    if (snap == INVALID_HANDLE_VALUE) { printf("THREAD_SNAP_ERR=%lu\n", GetLastError()); return 1; }
    THREADENTRY32 te;
    te.dwSize = sizeof(te);
    HookRec hooks[1024];
    int nhooks = 0, nfail = 0, nskip = 0;
    if (Thread32First(snap, &te)) {
        do {
            if (te.th32OwnerProcessID != pid) continue;
            if (nhooks >= 1024) { nskip++; continue; }
            HHOOK h = SetWindowsHookExA(WH_GETMESSAGE, proc, local, te.th32ThreadID);
            if (h) {
                hooks[nhooks].h = h; hooks[nhooks].tid = te.th32ThreadID; nhooks++;
                PostThreadMessageA(te.th32ThreadID, WM_NULL, 0, 0);
            } else {
                nfail++;
            }
        } while (Thread32Next(snap, &te));
    }
    CloseHandle(snap);
    printf("hooked=%d fail=%d skip=%d (pid=%lu)\n", nhooks, nfail, nskip, pid);
    for (int k = 0; k < nhooks; k++) printf("  hooked tid=%lu\n", hooks[k].tid);
    Sleep(6000);
    int loaded = dll_loaded_in_target(pid);
    printf("DLL_LOADED=%s\n", loaded == 1 ? "YES" : (loaded == 0 ? "NO" : "SNAP_ERR"));
    for (int k = 0; k < nhooks; k++) UnhookWindowsHookEx(hooks[k].h);
    return 0;
}