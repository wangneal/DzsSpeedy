/* modpaths.c — 32-bit module enumerator WITH full paths, focused on
 * inproc.dll / hook.dll / speedpatch / bsjl / Speeder and modules loaded
 * from non-system, non-game directories.
 * Usage: modpaths <pid>
 */
#include <windows.h>
#include <tlhelp32.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void shortname(const char *full, char *out, int n) {
    const char *slash = strrchr(full, '\\');
    /* szExePath in MODULEENTRY32 is a full path */
    strncpy(out, full, n - 1);
    out[n - 1] = 0;
    (void)slash;
}

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: modpaths <pid>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 0);

    /* get own module dir for compare */
    char game_candidates[4096];
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
    if (snap == INVALID_HANDLE_VALUE) { printf("SNAPSHOT_ERR=%lu\n", GetLastError()); return 1; }
    MODULEENTRY32 me;
    me.dwSize = sizeof(me);
    int count = 0;
    if (Module32First(snap, &me)) {
        do {
            count++;
            const char *name = me.szModule;
            int is_special = (_stricmp(name, "inproc.dll") == 0 || _stricmp(name, "hook.dll") == 0
                              || _stricmp(name, "dllprobe.dll") == 0 || _stricmp(name, "speedpatch32.dll") == 0
                              || _stricmp(name, "speedpatch64.dll") == 0);
            char base[MAX_PATH];
            GetModuleFileNameA(NULL, base, sizeof(base));
            if (is_special) {
                printf(">>>>> %s  [%s]\n", name, me.szExePath);
            }
            (void)game_candidates;
        } while (Module32Next(snap, &me));
    }
    printf("TOTAL=%d\n", count);
    CloseHandle(snap);
    return 0;
}