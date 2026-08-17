/* hooktest injector: mirrors src-bridge's inject_via_windows_hook path.
 * 1) LoadLibraryW(hookdll) locally, GetProcAddress(SP_TestHookProc)
 * 2) Toolhelp thread enumeration of the target
 * 3) SetWindowsHookExW(WH_GETMESSAGE) per thread
 * 4) PostThreadMessageW(WM_NULL) per accepted thread
 * 5) Poll marker files to prove DLL load + callback delivery
 */
#include <windows.h>
#include <tlhelp32.h>
#include <stdio.h>

static FILE *g_log = NULL;

static void out(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vprintf(fmt, ap);
    if (g_log) vfprintf(g_log, fmt, ap);
    va_end(ap);
    if (g_log) fflush(g_log);
    fflush(stdout);
}

int main(int argc, char **argv) {
    if (argc < 3) {
        printf("usage: inject <pid> <dllpath>\n");
        return 2;
    }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 10);
    const char *dll = argv[2];

    char logpath[MAX_PATH];
    GetTempPathA(MAX_PATH, logpath);
    snprintf(logpath + strlen(logpath), MAX_PATH - strlen(logpath),
             "hooktest-inject-%lu.log", pid);
    g_log = fopen(logpath, "w");
    out("inject pid=%lu dll=%s\n", pid, dll);

    wchar_t wdll[MAX_PATH];
    MultiByteToWideChar(CP_UTF8, 0, dll, -1, wdll, MAX_PATH);
    HMODULE hmod = LoadLibraryW(wdll);
    if (!hmod) {
        out("LoadLibraryW failed gle=%lu\n", GetLastError());
        return 3;
    }
    FARPROC proc = GetProcAddress(hmod, "SP_TestHookProc");
    if (!proc) proc = GetProcAddress(hmod, "SP_TestHookProc@12");
    if (!proc) {
        out("GetProcAddress failed gle=%lu\n", GetLastError());
        return 4;
    }
    out("local module ok\n");

    /* module already loaded in target? */
    int module_in_target = 0;
    HANDLE snap2 = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
    if (snap2 != INVALID_HANDLE_VALUE) {
        MODULEENTRY32W me;
        memset(&me, 0, sizeof(me));
        me.dwSize = sizeof(me);
        if (Module32FirstW(snap2, &me)) {
            do {
                if (_wcsicmp(me.szModule, L"hookdll.dll") == 0) module_in_target = 1;
            } while (Module32NextW(snap2, &me));
        }
        CloseHandle(snap2);
    }
    out("module_in_target_before=%d\n", module_in_target);

    /* enumerate target threads */
    DWORD tids[8192];
    int n = 0;
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
    if (snap == INVALID_HANDLE_VALUE) {
        out("CreateToolhelp32Snapshot(threads) failed gle=%lu\n", GetLastError());
        return 5;
    }
    THREADENTRY32 te;
    memset(&te, 0, sizeof(te));
    te.dwSize = sizeof(te);
    if (Thread32First(snap, &te)) {
        do {
            if (te.th32OwnerProcessID == pid && te.th32ThreadID != 0) {
                if (n < 8192) tids[n++] = te.th32ThreadID;
            }
        } while (Thread32Next(snap, &te));
    }
    CloseHandle(snap);
    out("candidate_threads=%d\n", n);

    /* install hooks */
    HHOOK hooks[8192];
    DWORD okTids[8192];
    int nok = 0;
    int fail57 = 0, fail5 = 0;
    DWORD firstErr = 0, firstErrTid = 0;
    for (int i = 0; i < n; i++) {
        HHOOK h = SetWindowsHookExW(WH_GETMESSAGE, (HOOKPROC)proc, hmod, tids[i]);
        if (h) {
            hooks[nok] = h;
            okTids[nok] = tids[i];
            nok++;
        } else {
            DWORD e = GetLastError();
            if (!firstErr) {
                firstErr = e;
                firstErrTid = tids[i];
            }
            if (e == 87) fail57++;
            if (e == 5) fail5++;
        }
    }
    out("installed=%d fail57=%d fail5=%d first_err=%lu(tid=%lu)\n",
        nok, fail57, fail5, firstErr, firstErrTid);
    out("accepted_tids:");
    for (int i = 0; i < nok; i++) out(" %lu", okTids[i]);
    out("\n");
    out("rejected_tids:");
    for (int i = 0; i < n; i++) {
        int found = 0;
        for (int j = 0; j < nok; j++) if (okTids[j] == tids[i]) { found = 1; break; }
        if (!found) out(" %lu", tids[i]);
    }
    out("\n");

    /* post wake-ups */
    int posted = 0;
    DWORD firstPostErr = 0;
    for (int i = 0; i < nok; i++) {
        if (PostThreadMessageW(okTids[i], WM_NULL, 0, 0)) {
            posted++;
        } else if (!firstPostErr) {
            firstPostErr = GetLastError();
        }
    }
    out("posted=%d first_post_err=%lu\n", posted, firstPostErr);

    /* poll markers for up to 20 s */
    char tmp[MAX_PATH];
    GetTempPathA(MAX_PATH, tmp);
    char dm[MAX_PATH], hp[MAX_PATH];
    snprintf(dm, sizeof(dm), "%shooktest-dllmain-%lu.txt", tmp, pid);
    snprintf(hp, sizeof(hp), "%shooktest-hookproc-%lu.txt", tmp, pid);
    int dllmainSeen = 0, hookprocSeen = 0;
    for (int i = 0; i < 200; i++) {
        if (!dllmainSeen &&
            GetFileAttributesA(dm) != INVALID_FILE_ATTRIBUTES) dllmainSeen = 1;
        if (!hookprocSeen &&
            GetFileAttributesA(hp) != INVALID_FILE_ATTRIBUTES) hookprocSeen = 1;
        if (dllmainSeen && hookprocSeen) break;
        Sleep(100);
    }
    out("dllmain_seen=%d hookproc_seen=%d\n", dllmainSeen, hookprocSeen);

    /* module presence after wait */
    module_in_target = 0;
    snap2 = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
    if (snap2 != INVALID_HANDLE_VALUE) {
        MODULEENTRY32W me;
        memset(&me, 0, sizeof(me));
        me.dwSize = sizeof(me);
        if (Module32FirstW(snap2, &me)) {
            do {
                if (_wcsicmp(me.szModule, L"hookdll.dll") == 0) module_in_target = 1;
            } while (Module32NextW(snap2, &me));
        }
        CloseHandle(snap2);
    }
    out("module_in_target_after=%d\n", module_in_target);

    /* unhook */
    int unhooked = 0;
    for (int i = 0; i < nok; i++) {
        if (UnhookWindowsHookEx(hooks[i])) unhooked++;
    }
    out("unhooked=%d\n", unhooked);
    out("DONE\n");
    if (g_log) fclose(g_log);
    return 0;
}
