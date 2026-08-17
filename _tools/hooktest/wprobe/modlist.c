/* modlist.c — 32-bit module enumerator. Lists all modules of a target
 * process (TH32CS_SNAPMODULE|TH32CS_SNAPMODULE32), so WOW64 visibility
 * limits are avoided. Also flags hook/speed/probe dlls.
 * Usage: modlist <pid>
 */
#include <windows.h>
#include <tlhelp32.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: modlist <pid>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 0);
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
    if (snap == INVALID_HANDLE_VALUE) {
        printf("SNAPSHOT_ERR=%lu\n", GetLastError());
        return 1;
    }
    MODULEENTRY32 me;
    me.dwSize = sizeof(me);
    int count = 0;
    if (Module32First(snap, &me)) {
        do {
            count++;
            int flag = (_stricmp(me.szModule, "hook.dll") == 0 || _stricmp(me.szModule, "dllprobe.dll") == 0
                        || _stricmp(me.szModule, "speedpatch32.dll") == 0 || _stricmp(me.szModule, "speedpatch64.dll") == 0);
            if (flag || count <= 300) {
                printf("%s%s\n", me.szModule, flag ? "   <=== TARGET" : "");
            }
        } while (Module32Next(snap, &me));
    }
    printf("TOTAL=%d\n", count);
    CloseHandle(snap);
    return 0;
}