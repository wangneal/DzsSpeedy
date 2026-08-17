/* Remote-load multiple DLLs into target and compare outcomes.
 * Usage: remload2 <pid>
 * Tries: (1) winhttp.dll (system, likely not loaded), (2) hookdll from D:, (3) hookdll from System32 copy.
 */
#include <windows.h>
#include <stdio.h>
#include <tlhelp32.h>

static int in_target(DWORD pid, const wchar_t *name) {
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
    if (snap == INVALID_HANDLE_VALUE) return 0;
    MODULEENTRY32W me; me.dwSize = sizeof(me);
    int found = 0;
    while (Module32NextW(snap, &me)) {
        if (_wcsicmp(me.szModule, name) == 0) { found = 1; break; }
    }
    CloseHandle(snap);
    return found;
}

static void remote_load(DWORD pid, const wchar_t *dllw, const char *label) {
    HANDLE h = OpenProcess(PROCESS_ALL_ACCESS, FALSE, pid);
    if (!h) { printf("[%s] OpenProcess failed err=%lu\n", label, GetLastError()); return; }
    FARPROC ll = GetProcAddress(GetModuleHandleA("kernel32.dll"), "LoadLibraryW");
    size_t len = (wcslen(dllw) + 1) * 2;
    void *mem = VirtualAllocEx(h, NULL, len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if (!mem) { printf("[%s] VirtualAllocEx failed err=%lu\n", label, GetLastError()); CloseHandle(h); return; }
    SIZE_T wr = 0;
    if (!WriteProcessMemory(h, mem, dllw, len, &wr)) { printf("[%s] WPM failed err=%lu\n", label, GetLastError()); VirtualFreeEx(h, mem, 0, MEM_RELEASE); CloseHandle(h); return; }
    HANDLE t = CreateRemoteThread(h, NULL, 0, (LPTHREAD_START_ROUTINE)ll, mem, 0, NULL);
    if (!t) { printf("[%s] CreateRemoteThread failed err=%lu\n", label, GetLastError()); VirtualFreeEx(h, mem, 0, MEM_RELEASE); CloseHandle(h); return; }
    DWORD w = WaitForSingleObject(t, 8000);
    DWORD exitc = 0;
    GetExitCodeThread(t, &exitc);
    int pres = in_target(pid, L"hookdll32.dll");
    if (wcsstr(dllw, L"winhttp") != NULL) pres = in_target(pid, L"winhttp.dll");
    printf("[%s] wait=%lu exit=0x%08lx in_target=%d\n", label, w, exitc, pres);
    fflush(stdout);
    VirtualFreeEx(h, mem, 0, MEM_RELEASE);
    CloseHandle(t);
    CloseHandle(h);
}

int main(int argc, char **argv) {
    if (argc < 2) { printf("usage: remload2 <pid>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 10);
    printf("pre winhttp_loaded=%d\n", in_target(pid, L"winhttp.dll"));
    remote_load(pid, L"winhttp.dll", "winhttp(system)");
    Sleep(1000);
    remote_load(pid, L"D:\\DzsSpeedy\\_tools\\hooktest\\hookdll32.dll", "hookdll(from D:)");
    Sleep(1000);
    remote_load(pid, L"C:\\Windows\\SysWOW64\\hookdll32.dll", "hookdll(from SysWOW64)");
    Sleep(1000);
    printf("pre asura.exe still there=%d\n", in_target(pid, L"asura.exe"));
    printf("DONE\n");
    return 0;
}