/* Remote LoadLibrary test: create a remote thread in the target that calls LoadLibraryW.
 * Usage: remload <pid> <dllpath>
 */
#include <windows.h>
#include <stdio.h>
#include <tlhelp32.h>

static int in_target(DWORD pid) {
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
    if (snap == INVALID_HANDLE_VALUE) return 0;
    MODULEENTRY32W me; me.dwSize = sizeof(me);
    int found = 0;
    while (Module32NextW(snap, &me)) {
        if (_wcsicmp(me.szModule, L"hookdll32.dll") == 0) { found = 1; break; }
    }
    CloseHandle(snap);
    return found;
}

int main(int argc, char **argv) {
    if (argc < 3) { printf("usage: remload <pid> <dllpath>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 10);
    const char *dll = argv[2];

    HANDLE h = OpenProcess(PROCESS_ALL_ACCESS, FALSE, pid);
    if (!h) { printf("OpenProcess failed err=%lu\n", GetLastError()); return 1; }

    FARPROC ll = GetProcAddress(GetModuleHandleA("kernel32.dll"), "LoadLibraryW");
    printf("LoadLibraryW=%p\n", ll);
    fflush(stdout);

    size_t len = (strlen(dll) + 1) * 2;
    void *mem = VirtualAllocEx(h, NULL, len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if (!mem) { printf("VirtualAllocEx failed err=%lu\n", GetLastError()); return 1; }
    wchar_t wdll[MAX_PATH];
    mbstowcs(wdll, dll, MAX_PATH);
    SIZE_T wr = 0;
    if (!WriteProcessMemory(h, mem, wdll, len, &wr)) { printf("WPM failed err=%lu\n", GetLastError()); return 1; }
    printf("alloc=%p wrote=%zu\n", mem, wr);
    fflush(stdout);

    HANDLE t = CreateRemoteThread(h, NULL, 0, (LPTHREAD_START_ROUTINE)ll, mem, 0, NULL);
    if (!t) { printf("CreateRemoteThread failed err=%lu\n", GetLastError()); return 1; }
    printf("remote thread=%p waiting\n", t);
    fflush(stdout);
    DWORD w = WaitForSingleObject(t, 10000);
    DWORD exitc = 0;
    GetExitCodeThread(t, &exitc);
    printf("wait=%lu exitcode=0x%08lx\n", w, exitc);
    fflush(stdout);

    /* module check right after, then again after 3s (loader may still be in DllMain) */
    for (int i = 0; i < 2; i++) {
        printf("dll_in_target[%d]=%d\n", i, in_target(pid));
        fflush(stdout);
        if (i == 0) Sleep(3000);
    }

    VirtualFreeEx(h, mem, 0, MEM_RELEASE);
    CloseHandle(t);
    CloseHandle(h);
    printf("DONE\n");
    return 0;
}