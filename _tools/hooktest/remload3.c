/* Pinpoint where the loader lockdown sits.
 * Remote-call sequence in target:
 *  1. GetCurrentProcessId           -> thread execution check (expect = pid)
 *  2. GetModuleHandleW(winhttp)     -> pure PEB lookup check (expect non-0)
 *  3. LoadLibraryW(winhttp)         -> loader call (already-loaded name)
 *  4. LoadLibraryA(hookdll D:)      -> loader call (new module)
 * Usage: remload3 <pid>
 */
#include <windows.h>
#include <stdio.h>

static void remote_call(DWORD pid, FARPROC fn, void *arg, const char *label) {
    HANDLE h = OpenProcess(PROCESS_ALL_ACCESS, FALSE, pid);
    if (!h) { printf("[%s] OpenProcess failed err=%lu\n", label, GetLastError()); return; }
    void *mem = NULL;
    SIZE_T n = 0;
    if (arg) {
        size_t len = (strlen((char *)arg) + 1) * 2;
        mem = VirtualAllocEx(h, NULL, len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        if (!mem) { printf("[%s] alloc failed err=%lu\n", label, GetLastError()); CloseHandle(h); return; }
        wchar_t w[512];
        mbstowcs(w, (char *)arg, 512);
        WriteProcessMemory(h, mem, w, len, &n);
    }
    HANDLE t = CreateRemoteThread(h, NULL, 0, (LPTHREAD_START_ROUTINE)fn, mem, 0, NULL);
    if (!t) { printf("[%s] CreateRemoteThread failed err=%lu\n", label, GetLastError()); if (mem) VirtualFreeEx(h, mem, 0, MEM_RELEASE); CloseHandle(h); return; }
    WaitForSingleObject(t, 10000);
    DWORD exitc = 0;
    GetExitCodeThread(t, &exitc);
    printf("[%s] exit=0x%08lx (%ld)\n", label, exitc, (long)exitc);
    fflush(stdout);
    if (mem) VirtualFreeEx(h, mem, 0, MEM_RELEASE);
    CloseHandle(t);
    CloseHandle(h);
}

int main(int argc, char **argv) {
    if (argc < 2) { printf("usage: remload3 <pid>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 10);
    HMODULE k32 = GetModuleHandleA("kernel32.dll");
    FARPROC gpid = GetProcAddress(k32, "GetCurrentProcessId");
    FARPROC gmh = GetProcAddress(k32, "GetModuleHandleW");
    FARPROC llw = GetProcAddress(k32, "LoadLibraryW");
    FARPROC lla = GetProcAddress(k32, "LoadLibraryA");
    printf("addrs: gpid=%p gmh=%p llw=%p lla=%p\n", gpid, gmh, llw, lla);
    remote_call(pid, gpid, NULL, "GetCurrentProcessId");
    Sleep(400);
    remote_call(pid, gmh, (void *)"winhttp.dll", "GetModuleHandleW(winhttp)");
    Sleep(400);
    remote_call(pid, llw, (void *)"winhttp.dll", "LoadLibraryW(winhttp)");
    Sleep(400);
    remote_call(pid, lla, (void *)"D:\\DzsSpeedy\\_tools\\hooktest\\hookdll32.dll", "LoadLibraryA(hookdll D:)");
    printf("DONE\n");
    return 0;
}