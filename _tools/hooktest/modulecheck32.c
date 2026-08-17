/* 32-bit module integrity scan: compare .text of target's modules with on-disk files.
 * Usage: modulecheck <pid>
 */
#include <windows.h>
#include <stdio.h>
#include <tlhelp32.h>

static unsigned long long hash16(const unsigned char *p, size_t n) {
    unsigned long long h = 1469598103934665603ULL;
    for (size_t i = 0; i < n; i++) { h ^= p[i]; h *= 1099511628211ULL; }
    return h;
}

int main(int argc, char **argv) {
    if (argc < 2) { printf("usage: modulecheck <pid>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 10);

    HANDLE h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, FALSE, pid);
    if (!h) { printf("OpenProcess failed err=%lu\n", GetLastError()); return 1; }

    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
    if (snap == INVALID_HANDLE_VALUE) { printf("snapshot failed\n"); return 1; }
    MODULEENTRY32W me; me.dwSize = sizeof(me);
    int nchecked = 0, ndiff = 0;
    while (Module32NextW(snap, &me)) {
        wchar_t fn[MAX_PATH];
        swprintf(fn, MAX_PATH, L"%s", me.szExePath);
        HANDLE f = CreateFileW(fn, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
        if (f == INVALID_HANDLE_VALUE) continue;
        DWORD sz = GetFileSize(f, NULL);
        if (sz == INVALID_FILE_SIZE || sz == 0) { CloseHandle(f); continue; }
        unsigned char *buf = (unsigned char *)malloc(sz);
        DWORD rd = 0;
        ReadFile(f, buf, sz, &rd, NULL);
        CloseHandle(f);
        if (rd != sz) { free(buf); continue; }

        /* parse PE */
        unsigned char *p = buf;
        if (rd < 0x40 || p[0] != 'M' || p[1] != 'Z') { free(buf); continue; }
        DWORD pe = *(DWORD *)(p + 0x3c);
        if (pe + 0x18 > rd || p[pe] != 'P' || p[pe + 1] != 'E') { free(buf); continue; }
        WORD nsects = *(WORD *)(p + pe + 6);
        DWORD opt = pe + 24;
        WORD magic = *(WORD *)(p + opt);
        DWORD sect_off = opt + (magic == 0x20b ? 240 : 224);
        DWORD text_rva = 0, text_size = 0;
        for (int i = 0; i < nsects; i++) {
            DWORD so = sect_off + i * 40;
            if (so + 40 > rd) break;
            if (memcmp(p + so, ".text", 5) == 0) {
                text_size = *(DWORD *)(p + so + 8);
                text_rva = *(DWORD *)(p + so + 12);
            }
        }
        free(buf);
        if (!text_size || text_size > (4u << 20)) continue;

        /* read from target */
        unsigned char *tb = (unsigned char *)malloc(text_size);
        SIZE_T got = 0;
        BOOL ok = ReadProcessMemory(h, (LPCVOID)((ULONG_PTR)me.modBaseAddr + text_rva), tb, text_size, &got);

        if (ok && got == text_size) {
            /* read same range from file via ITS rva->offset: section header had raw ptr; simpler: re-read file fully. */
            HANDLE f2 = CreateFileW(fn, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
            DWORD sz2 = GetFileSize(f2, NULL);
            unsigned char *fbuf = (unsigned char *)malloc(sz2);
            DWORD rd2 = 0;
            ReadFile(f2, fbuf, sz2, &rd2, NULL);
            CloseHandle(f2);
            /* find .text file offset */
            DWORD raw_off = 0, raw_size = 0;
            if (rd2 == sz2 && sz2 >= 0x40 && fbuf[0] == 'M' && fbuf[1] == 'Z') {
                DWORD pe2 = *(DWORD *)(fbuf + 0x3c);
                WORD ns2 = *(WORD *)(fbuf + pe2 + 6);
                DWORD opt2 = pe2 + 24;
                WORD mg2 = *(WORD *)(fbuf + opt2);
                DWORD so2 = opt2 + (mg2 == 0x20b ? 240 : 224);
                for (int i = 0; i < ns2; i++) {
                    DWORD s = so2 + i * 40;
                    if (memcmp(fbuf + s, ".text", 5) == 0) {
                        raw_size = *(DWORD *)(fbuf + s + 16);
                        raw_off = *(DWORD *)(fbuf + s + 20);
                    }
                }
                if (raw_off && raw_size >= text_size) {
                    unsigned long long h1 = hash16(tb, text_size);
                    unsigned long long h2 = hash16(fbuf + raw_off, text_size);
                    int same = h1 == h2;
                    nchecked++;
                    if (!same) ndiff++;
                    if (!same && nchecked <= 3) {
                        printf("  [dbg] target first16: ");
                        for (int i = 0; i < 16; i++) printf("%02x ", tb[i]);
                        printf("\n  [dbg] file   first16: ");
                        for (int i = 0; i < 16; i++) printf("%02x ", fbuf[raw_off + i]);
                        printf("\n");
                    }
                    printf("%-24s text=0x%08x %s  (h %016llx vs %016llx)\n",
                           me.szModule, text_size, same ? "MATCH" : "*** DIFF ***",
                           h1, h2);
                }
                free(fbuf);
            } else if (fbuf) free(fbuf);
        }
        free(tb);
    }
    CloseHandle(snap);
    CloseHandle(h);
    printf("checked=%d diff=%d\n", nchecked, ndiff);
    return 0;
}