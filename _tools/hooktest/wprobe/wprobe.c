/* wprobe.c — decisive injection probe driver (x86/x64 build).
 * Usage: wprobe <pid> [msg|keyboard|cbt|shell] [visible|any]
 *
 * Mimics the two rival strategies against a live target:
 *   - bsjl-style: SetWindowsHookExW onto the window-owner thread only
 *   - DzsSpeedy-style: additionally PostThreadMessageW(WM_NULL) wake
 * Then checks whether the probe DLL actually got loaded into the target
 * (module snapshot). Prints one verdict line, grep-able:
 *   WINDOW=<title>|none TID=<tid> HOOK=OK|ERR:<code> DLL_LOADED=YES|NO
 */
#include <windows.h>
#include <tlhelp32.h>
#include <stdio.h>
#include <string.h>

static DWORD g_target_pid = 0;
static HWND  g_best = NULL;
static BOOL  g_prefer_visible = TRUE;

static BOOL CALLBACK EnumProc(HWND hwnd, LPARAM lparam) {
    DWORD pid = 0;
    GetWindowThreadProcessId(hwnd, &pid);
    if (pid != g_target_pid) return TRUE;
    if (!IsWindowVisible(hwnd) && g_prefer_visible) return TRUE; /* keep looking */
    g_best = hwnd;
    return FALSE; /* found */
}

static int dll_loaded_in_target(DWORD pid, const char *dll_base) {
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
    if (snap == INVALID_HANDLE_VALUE) return -1;
    MODULEENTRY32 me;
    me.dwSize = sizeof(me);
    int found = 0;
    if (Module32First(snap, &me)) {
        do {
            if (_stricmp(me.szModule, dll_base) == 0) { found = 1; break; }
        } while (Module32Next(snap, &me));
    }
    CloseHandle(snap);
    return found;
}

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: wprobe <pid> [msg|keyboard|cbt|shell] [visible|any]\n"); return 2; }
    g_target_pid = (DWORD)strtoul(argv[1], NULL, 0);
    int idHook = WH_GETMESSAGE;
    if (argc > 2) {
        if (!strcmp(argv[2], "keyboard")) idHook = WH_KEYBOARD;
        else if (!strcmp(argv[2], "cbt")) idHook = WH_CBT;
        else if (!strcmp(argv[2], "shell")) idHook = WH_SHELL;
        else if (!strcmp(argv[2], "callwndproc")) idHook = WH_CALLWNDPROC;
        else if (!strcmp(argv[2], "mouse")) idHook = WH_MOUSE;
        else if (!strcmp(argv[2], "debug")) idHook = WH_DEBUG;
        else if (!strcmp(argv[2], "foregroundidle")) idHook = WH_FOREGROUNDIDLE;
        else if (!strcmp(argv[2], "msgfilter")) idHook = WH_MSGFILTER;
        else if (!strcmp(argv[2], "sysmsgfilter")) idHook = WH_SYSMSGFILTER;
        else if (!strcmp(argv[2], "hardware")) idHook = WH_HARDWARE;
        else if (!strcmp(argv[2], "journalplayback")) idHook = WH_JOURNALPLAYBACK;
    }
    if (argc > 3 && !strcmp(argv[3], "any")) g_prefer_visible = FALSE;

    EnumWindows(EnumProc, 0);

    char title[256] = "none";
    DWORD tid = 0;
    if (g_best) {
        GetWindowTextA(g_best, title, sizeof(title));
        tid = GetWindowThreadProcessId(g_best, NULL);
    }
    if (tid == 0) {
        /* no window: fall back to the first process thread (toolhelp) */
        HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if (snap != INVALID_HANDLE_VALUE) {
            THREADENTRY32 te;
            te.dwSize = sizeof(te);
            if (Thread32First(snap, &te)) {
                do {
                    if (te.th32OwnerProcessID == g_target_pid) { tid = te.th32ThreadID; break; }
                } while (Thread32Next(snap, &te));
            }
            CloseHandle(snap);
        }
    }

    char dll_path[MAX_PATH];
    GetModuleFileNameA(NULL, dll_path, sizeof(dll_path));
    if (argc > 4) {
        strncpy(dll_path, argv[4], sizeof(dll_path) - 1);
        dll_path[sizeof(dll_path) - 1] = 0;
    } else {
        char *slash = strrchr(dll_path, '\\');
        if (slash) strcpy(slash + 1, "dllprobe.dll");
    }
    char dll_base[MAX_PATH] = "dllprobe.dll";
    {
        char *slash = strrchr(dll_path, '\\');
        if (slash) strcpy(dll_base, slash + 1);
    }
    HMODULE local = LoadLibraryA(dll_path);
    if (!local) { printf("WINDOW=%s|none TID=%lu LOCAL_LOAD=ERR:%lu DLL_LOADED=NO\n", title, tid, GetLastError()); return 1; }
HOOKPROC proc = NULL;
    if (argc > 5) {
        proc = (HOOKPROC)GetProcAddress(local, (LPCSTR)(DWORD)atoi(argv[5])); /* ordinal */
        printf("proc=ordinal:%s\n", argv[5]);
    } else {
        const char *names[] = { "ProbeHookProc@12", "ProbeHookProc", "_ProbeHookProc@12", "_ProbeHookProc" };
        for (int k = 0; k < 4 && !proc; k++) proc = (HOOKPROC)GetProcAddress(local, names[k]);
    }
    if (!proc) { printf("WINDOW=%s|none TID=%lu PROC=ERR DLL_LOADED=NO\n", title, tid); return 1; }

    HHOOK hook = SetWindowsHookExA(idHook, proc, local, tid);
    if (!hook) {
        printf("WINDOW=%s TID=%lu HOOK=ERR:%lu DLL_LOADED=NO\n", title, tid, GetLastError());
        return 1;
    }
    /* DzsSpeedy-style wake on the hooked thread */
    PostThreadMessageA(tid, WM_NULL, 0, 0);
    Sleep(5000);
    int loaded = dll_loaded_in_target(g_target_pid, dll_base);
    printf("WINDOW=%s TID=%lu HOOK=OK DLL_LOADED=%s\n",
           title, tid, loaded == 1 ? "YES" : (loaded == 0 ? "NO" : "SNAP_ERR"));
    UnhookWindowsHookEx(hook);
    return 0;
}
