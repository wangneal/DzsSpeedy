/* Relocation-aware .text diff for a target process.
 * For each 4-byte aligned position: if target value looks like a pointer into any
 * loaded module range (relocation), allow difference; otherwise bytes must match.
 * Usage: modscan <pid>
 */
#include <windows.h>
#include <stdio.h>
#include <tlhelp32.h>

#define MAXMOD 512
typedef struct { ULONG_PTR base; ULONG_PTR end; } RANGE;
static RANGE ranges[MAXMOD];
static int nranges = 0;

static int is_ptr(ULONG_PTR v) {
    for (int i = 0; i < nranges; i++)
        if (v >= ranges[i].base && v < ranges[i].end) return 1;
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 2) { printf("usage: modscan <pid>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 10);

    HANDLE snap = INVALID_HANDLE_VALUE;
    for (int attempt = 0; attempt < 5; attempt++) {
        snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
        if (snap != INVALID_HANDLE_VALUE) break;
        Sleep(300);
    }
    if (snap == INVALID_HANDLE_VALUE) { printf("snapshot failed err=%lu\n", GetLastError()); return 1; }
    MODULEENTRY32W me; me.dwSize = sizeof(me);
    while (Module32NextW(snap, &me) && nranges < MAXMOD) {
        ranges[nranges].base = (ULONG_PTR)me.modBaseAddr;
        ranges[nranges].end = ranges[nranges].base + me.modBaseSize;
        nranges++;
    }
    if (nranges >= MAXMOD) { printf("range overflow\n"); return 1; }
    printf("modules=%d\n", nranges);

    HANDLE h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, FALSE, pid);
    if (!h) { printf("OpenProcess failed err=%lu\n", GetLastError()); return 1; }

    /* scan specific modules: user32, ntdll, kernelbase, winmm, d3d9 */
    const wchar_t *targets[] = { L"user32.dll", L"ntdll.dll", L"kernelbase.dll", L"winmm.dll", L"d3d9.dll", NULL };
    const wchar_t *prefixes[] = { L"kernel32.dll", L"gdi32.dll", L"imm32.dll", NULL };

    for (int t = 0; targets[t]; t++) {
        MODULEENTRY32W me2; me2.dwSize = sizeof(me2);
        HANDLE snap2 = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
        int found = 0;
        while (Module32NextW(snap2, &me2)) {
            if (_wcsicmp(me2.szModule, targets[t]) == 0) { found = 1; break; }
        }
        CloseHandle(snap2);
        if (!found) { printf("%-16s not loaded\n", targets[t]); continue; }

        wchar_t path[MAX_PATH];
        swprintf(path, MAX_PATH, L"%s", me2.szExePath);
        HANDLE f = CreateFileW(path, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
        if (f == INVALID_HANDLE_VALUE) { printf("%-16s file open failed\n", targets[t]); continue; }
        DWORD fsz = GetFileSize(f, NULL);
        unsigned char *fb = (unsigned char *)malloc(fsz);
        DWORD rd = 0;
        ReadFile(f, fb, fsz, &rd, NULL);
        CloseHandle(f);
        if (rd != fsz) { printf("%-16s file read failed\n", targets[t]); free(fb); continue; }

        DWORD pe = *(DWORD *)(fb + 0x3c);
        WORD ns = *(WORD *)(fb + pe + 6);
        DWORD opt = pe + 24;
        WORD mg = *(WORD *)(fb + opt);
        DWORD so = opt + (mg == 0x20b ? 240 : 224);
        DWORD text_rva = 0, text_size = 0, raw_off = 0, raw_size = 0;
        for (int i = 0; i < ns; i++) {
            DWORD s = so + i * 40;
            if (memcmp(fb + s, ".text", 5) == 0) {
                text_size = *(DWORD *)(fb + s + 8);
                text_rva = *(DWORD *)(fb + s + 12);
                raw_size = *(DWORD *)(fb + s + 16);
                raw_off = *(DWORD *)(fb + s + 20);
            }
        }
        if (!text_size) { printf("%-16s no .text\n", targets[t]); free(fb); continue; }
        if (text_size > raw_size) text_size = raw_size ? raw_size : text_size;

        unsigned char *tb = (unsigned char *)malloc(text_size);
        SIZE_T got = 0;
        BOOL ok = ReadProcessMemory(h, (LPCVOID)((ULONG_PTR)me2.modBaseAddr + text_rva), tb, text_size, &got);
        if (!ok || got != text_size) { printf("%-16s target read failed (err=%lu)\n", targets[t], GetLastError()); free(fb); free(tb); continue; }

        int ndiff = 0, nshown = 0;
        for (DWORD i = 0; i + 4 <= text_size; i += 1) {
            if (tb[i] == fb[raw_off + i]) continue;
            /* possible 4-byte pointer difference (unaligned or aligned) */
            if (i + 4 <= text_size) {
                ULONG_PTR tv, fv;
                memcpy(&tv, tb + i, 4);
                memcpy(&fv, fb + raw_off + i, 4);
                if (is_ptr(tv)) continue;      /* target has pointer, file has placeholder -> relocation */
                /* target differs and is not a pointer: real patch OR padding */
            }
            if (nshown < 6) {
                printf("%-16s .text+0x%06x: target=", targets[t], i);
                for (int k = 0; k < 16 && (i + k) < text_size; k++) printf("%02x ", tb[i + k]);
                printf(" file=");
                for (int k = 0; k < 16 && (i + k) < text_size; k++) printf("%02x ", fb[raw_off + i + k]);
                printf("\n");
                nshown++;
            }
            ndiff++;
        }
        printf("%-16s .text=0x%08x nonreloc_diffs=%d\n", targets[t], text_size, ndiff);
        free(fb);
        free(tb);
    }
    CloseHandle(h);
    return 0;
}