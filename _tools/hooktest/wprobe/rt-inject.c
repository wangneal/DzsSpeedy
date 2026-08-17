/* rt-inject.c — remote-thread injection probe (x86).
 * Usage: rt-inject <pid> <dll-path> [alloc_method]
 * Opens the target, shoves the DLL path into it via VirtualAllocEx +
 * WriteProcessMemory, then CreateRemoteThread(LoadLibraryW). Waits a few
 * seconds and checks whether the DLL actually got loaded (module snapshot).
 * Prints RT_* verdict lines. The DLL is dllprobe.dll unless overridden.
 */
#include <windows.h>
#include <tlhelp32.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

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
    if (argc < 3) { fprintf(stderr, "usage: rt-inject <pid> <dll-path>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 0);
    const char *dll = argv[2];
    const char *dll_base = strrchr(dll, '\\') ? strrchr(dll, '\\') + 1 : dll;

    if (dll_loaded_in_target(pid, dll_base) == 1) {
        printf("ALREADY_LOADED=YES %s\n", dll_base);
        return 0;
    }

    HANDLE h = OpenProcess(PROCESS_CREATE_THREAD | PROCESS_QUERY_INFORMATION |
                           PROCESS_VM_OPERATION | PROCESS_VM_WRITE | PROCESS_VM_READ, FALSE, pid);
    if (!h) { printf("OPEN_ERR=%lu\n", GetLastError()); return 1; }

    size_t len = strlen(dll) + 1;
    void *remote = VirtualAllocEx(h, NULL, len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if (!remote) { printf("ALLOC_ERR=%lu\n", GetLastError()); CloseHandle(h); return 1; }
    if (!WriteProcessMemory(h, remote, dll, len, NULL)) {
        printf("WRITE_ERR=%lu\n", GetLastError()); VirtualFreeEx(h, remote, 0, MEM_RELEASE); CloseHandle(h); return 1;
    }
    HMODULE k32 = GetModuleHandleA("kernel32.dll");
    FARPROC loadlib = GetProcAddress(k32, "LoadLibraryW");
    HANDLE thr = CreateRemoteThread(h, NULL, 0, (LPTHREAD_START_ROUTINE)loadlib, remote, 0, NULL);
    if (!thr) {
        printf("RT_ERR=%lu\n", GetLastError());
        VirtualFreeEx(h, remote, 0, MEM_RELEASE); CloseHandle(h); return 1;
    }
    printf("RT_CREATED\n");
    DWORD wait = WaitForSingleObject(thr, 8000);
    printf("RT_WAIT=%s\n", wait == WAIT_OBJECT_0 ? "OK" : (wait == WAIT_TIMEOUT ? "TIMEOUT" : "ERR"));
    DWORD ret = 0;
    GetExitCodeThread(thr, &ret);
    printf("RT_EXIT=0x%08lX %s\n", ret, ret ? "(nonzero=load ok?)" : "(zero=load failed?)");
    CloseHandle(thr);
    VirtualFreeEx(h, remote, 0, MEM_RELEASE);
    CloseHandle(h);
    Sleep(1000);
    int loaded = dll_loaded_in_target(pid, dll_base);
    printf("DLL_LOADED=%s\n", loaded == 1 ? "YES" : (loaded == 0 ? "NO" : "SNAP_ERR"));
    return 0;
}