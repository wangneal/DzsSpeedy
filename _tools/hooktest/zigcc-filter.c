/* zigcc-filter.c — rustc linker-driver shim: route rustc's GNU-style link
 * command line through `zig cc -target <TARGET>`, adapting arguments that
 * break zig's driver:
 *   - `-Wl,--large-address-aware` (zig's parser rejects it for 32-bit)
 *   - `-Wl,-Bdynamic` must become `-Wl,-Bstatic`: zig resolves Windows import
 *     libraries by name, and in dynamic mode it only looks for `<name>.dll`
 *     (no `-L` dirs carry those), whereas rustup's mingw self-contained
 *     import libs are named `lib<name>.a`.
 * `-nodefaultlibs` is kept: rustc supplies every needed `-l` itself and the
 * `-L` dir (RUSTFLAGS) resolves them all.
 *
 * Build (host tool; zig compiles for the native msvc ABI, fine for a shim):
 *   zig cc -O2 -DTARGET=\"x86_64-windows-gnu\" zigcc-filter.c -o zigcc64.exe
 *   zig cc -O2 -DTARGET=\"x86-windows-gnu\"     zigcc-filter.c -o zigcc32.exe
 */
#include <windows.h>
#include <stdio.h>
#include <string.h>

#ifndef TARGET
#define TARGET "x86_64-windows-gnu"
#endif

#define ZIG "C:\\ProgramData\\chocolatey\\bin\\zig.exe"

int main(int argc, char **argv) {
    static char cmd[32768];
    char *p = cmd;
    int i;
    p += sprintf(p, "\"%s\" cc -target %s -nostartfiles", ZIG, TARGET);
    for (i = 1; i < argc; i++) {
        const char *a = argv[i];
        if (!strcmp(a, "-Wl,--large-address-aware")) continue;
        if (!strcmp(a, "-Wl,-Bdynamic")) a = "-Wl,-Bstatic";
        /* zig 0.16 crashes (0xc0000005) on the exact-filename form;
         * libpthread.a exists in the -L dir, plain -lpthread resolves it. */
        if (!strcmp(a, "-l:libpthread.a")) a = "-lpthread";
        p += sprintf(p, " \"%s\"", a);
    }
    STARTUPINFOA si;
    PROCESS_INFORMATION pi;
    memset(&si, 0, sizeof si);
    si.cb = sizeof si;
    memset(&pi, 0, sizeof pi);
    if (!CreateProcessA(ZIG, cmd, NULL, NULL, TRUE, 0, NULL, NULL, &si, &pi)) {
        fprintf(stderr, "zigcc: CreateProcess failed: %lu\n", GetLastError());
        return 127;
    }
    WaitForSingleObject(pi.hProcess, INFINITE);
    DWORD code = 1;
    GetExitCodeProcess(pi.hProcess, &code);
    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);
    return (int)code;
}