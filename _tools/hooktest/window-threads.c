/* Enumerate top-level windows of a process; write owner thread ids.
 * Usage: window-threads <pid> [outfile]  (default %TEMP%\hooktest-windows-<pid>.txt)
 */
#include <windows.h>
#include <stdio.h>

typedef struct { DWORD pid; FILE *f; } CTX;

static BOOL CALLBACK EnumProc(HWND hwnd, LPARAM lp) {
    CTX *c = (CTX *)lp;
    DWORD pid = 0, tid = GetWindowThreadProcessId(hwnd, &pid);
    if (pid == c->pid) {
        BOOL vis = IsWindowVisible(hwnd);
        char cls[256] = {0};
        GetClassNameA(hwnd, cls, sizeof(cls));
        fprintf(c->f, "hwnd=%p tid=%lu visible=%d class=%s\n", hwnd, tid, vis, cls);
        fflush(c->f);
    }
    return TRUE;
}

int main(int argc, char **argv) {
    if (argc < 2) { printf("usage: window-threads <pid>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 10);
    char path[MAX_PATH];
    GetTempPathA(MAX_PATH, path);
    char fname[MAX_PATH];
    snprintf(fname, sizeof(fname), "%shooktest-windows-%lu.txt", path, pid);
    FILE *f = fopen(fname, "w");
    if (!f) { printf("cannot open %s\n", fname); return 3; }
    CTX c = { pid, f };
    EnumWindows(EnumProc, (LPARAM)&c);
    fclose(f);
    printf("done, out=%s\n", fname);
    return 0;
}