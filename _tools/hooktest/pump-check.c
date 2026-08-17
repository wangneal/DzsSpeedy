/* Check whether a process's window threads are currently pumping messages:
 * SendMessageTimeoutW(WM_NULL) — succeeds only if the owner thread pumps.
 * Usage: pump-check <pid>
 */
#include <windows.h>
#include <stdio.h>

typedef struct { DWORD pid; } CTX;

static BOOL CALLBACK EnumProc(HWND hwnd, LPARAM lp) {
    CTX *c = (CTX *)lp;
    DWORD pid = 0;
    DWORD tid = GetWindowThreadProcessId(hwnd, &pid);
    if (pid != c->pid) return TRUE;
    DWORD_PTR res = 0;
    DWORD r = SendMessageTimeoutW(hwnd, WM_NULL, 0, 0,
                                  SMTO_ABORTIFHUNG | SMTO_NORMAL, 1500, &res);
    char cls[256] = {0};
    GetClassNameA(hwnd, cls, sizeof(cls));
    BOOL vis = IsWindowVisible(hwnd);
    printf("hwnd=%p tid=%lu visible=%d class=%s pump=%s\n",
           hwnd, tid, vis, cls, r ? "YES" : "NO/HUNG");
    fflush(stdout);
    return TRUE;
}

int main(int argc, char **argv) {
    if (argc < 2) { printf("usage: pump-check <pid>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 10);
    CTX c = { pid };
    EnumWindows(EnumProc, (LPARAM)&c);
    return 0;
}