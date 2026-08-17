/* Compare key hook-delivery functions in a target process vs local copies.
 * If Asura in-process-patched user32/ntdll, bytes will differ.
 * Usage: hookcheck <pid>
 */
#include <windows.h>
#include <stdio.h>
#include <tlhelp32.h>

typedef struct { const char *mod; const char *exp; } PAIR;

static const PAIR pairs[] = {
    {"user32.dll", "__ClientLoadLibrary"},
    {"user32.dll", "__ClientHookProc"},
    {"user32.dll", "__ClientGetMessageHookProc"},
    {"user32.dll", "__ClientCallWinEventProc"},
    {"user32.dll", "__ClientSetWindowLongPtr"},
    {"ntdll.dll", "LdrLoadDll"},
    {"ntdll.dll", "NtMapViewOfSection"},
    {"ntdll.dll", "LdrUnloadDll"},
    {"kernelbase.dll", "LoadLibraryExW"},
    {"kernelbase.dll", "LoadLibraryW"},
    {NULL, NULL}
};

static void dump(const char *tag, const unsigned char *p) {
    printf("%-28s ", tag);
    for (int i = 0; i < 16; i++) printf("%02x ", p[i]);
    printf("\n");
}

int main(int argc, char **argv) {
    if (argc < 2) { printf("usage: hookcheck <pid>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 10);

    /* target module bases */
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
    if (snap == INVALID_HANDLE_VALUE) { printf("snapshot failed err=%lu\n", GetLastError()); return 1; }
    MODULEENTRY32W me; me.dwSize = sizeof(me);
    ULONG_PTR base[3] = { 0, 0, 0 };   /* user32, ntdll, kernelbase */
    while (Module32NextW(snap, &me)) {
        if (_wcsicmp(me.szModule, L"user32.dll") == 0) base[0] = (ULONG_PTR)me.modBaseAddr;
        else if (_wcsicmp(me.szModule, L"ntdll.dll") == 0) base[1] = (ULONG_PTR)me.modBaseAddr;
        else if (_wcsicmp(me.szModule, L"kernelbase.dll") == 0) base[2] = (ULONG_PTR)me.modBaseAddr;
    }
    CloseHandle(snap);
    printf("target bases: user32=%p ntdll=%p kernelbase=%p\n",
           (void *)base[0], (void *)base[1], (void *)base[2]);

    HANDLE h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, FALSE, pid);
    if (!h) { printf("OpenProcess failed err=%lu\n", GetLastError()); return 1; }

    for (int i = 0; pairs[i].mod; i++) {
        int idx = _stricmp(pairs[i].mod, "user32.dll") == 0 ? 0 :
                  _stricmp(pairs[i].mod, "ntdll.dll") == 0 ? 1 : 2;
        HMODULE lm = GetModuleHandleA(pairs[i].mod);
        if (!lm || !base[idx]) { printf("%-28s module not found\n", pairs[i].mod); continue; }
        void *local = (void *)GetProcAddress(lm, pairs[i].exp);
        if (!local) { printf("%-28s export not found\n", pairs[i].exp); continue; }
        ULONG_PTR off = (ULONG_PTR)local - (ULONG_PTR)lm;
        unsigned char lb[16] = { 0 }, tb[16] = { 0 };
        memcpy(lb, local, 16);
        SIZE_T rd = 0;
        BOOL ok = ReadProcessMemory(h, (LPCVOID)(base[idx] + off), tb, 16, &rd);
        char tag[64];
        snprintf(tag, sizeof(tag), "%s!%s", pairs[i].mod, pairs[i].exp);
        printf("%-28s off=0x%08llx local=", tag, (unsigned long long)off);
        dump("", lb);
        printf("  target(read_ok=%d)=", ok);
        dump("", tb);
    }
    CloseHandle(h);
    return 0;
}