/* Precise relocation-aware .text comparison using the file's .reloc table.
 * Any remaining difference = in-memory patch (protection).
 * Usage: modscan3 <pid>
 */
#include <windows.h>
#include <stdio.h>
#include <tlhelp32.h>

#define MAXFIX 200000
static DWORD fixups[MAXFIX];
static int nfix = 0;

static int is_fixup(DWORD rva) {
    /* fixup entries cover 4 bytes at rva; accept rva in [f, f+3] */
    int lo = 0, hi = nfix - 1;
    while (lo <= hi) {
        int mid = (lo + hi) / 2;
        if (rva < fixups[mid]) hi = mid - 1;
        else if (rva > fixups[mid] + 3) lo = mid + 1;
        else return 1;
    }
    return 0;
}

static void parse_relocs(const unsigned char *p, DWORD sz, DWORD pe, WORD nsects, DWORD sect_off) {
    for (int i = 0; i < nsects; i++) {
        DWORD s = sect_off + i * 40;
        if (s + 40 > sz) break;
        const char *name = (const char *)(p + s);
        if (memcmp(name, ".reloc", 6) != 0) continue;
        DWORD vsz = *(DWORD *)(p + s + 8);
        DWORD vaddr = *(DWORD *)(p + s + 12);
        DWORD rsz = *(DWORD *)(p + s + 16);
        DWORD roff = *(DWORD *)(p + s + 20);
        if (roff + rsz > sz || vsz < 8) continue;
        DWORD end = roff + (rsz < vsz ? rsz : vsz);
        DWORD off = roff;
        while (off + 8 <= end) {
            DWORD page = *(DWORD *)(p + off);
            DWORD cnt = *(DWORD *)(p + off + 4);
            if (!cnt) break;
            for (DWORD e = off + 8; e + 2 <= off + cnt && e + 2 <= end; e += 2) {
                WORD ent = *(WORD *)(p + e);
                WORD type = ent >> 12;
                WORD rel = ent & 0xfff;
                if (type == 3 || type == 0xa) {       /* HIGHLOW / DIR64 */
                    if (nfix < MAXFIX) fixups[nfix++] = page + rel;
                }
            }
            off += cnt;
        }
    }
}

int main(int argc, char **argv) {
    if (argc < 2) { printf("usage: modscan3 <pid>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 10);

    HANDLE h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, FALSE, pid);
    if (!h) { printf("OpenProcess failed err=%lu\n", GetLastError()); return 1; }

    const wchar_t *mods[] = { L"ntdll.dll", L"user32.dll", L"kernelbase.dll", L"kernel32.dll" };
    for (int m = 0; m < 4; m++) {
        MODULEENTRY32W me; me.dwSize = sizeof(me);
        HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
        int found = 0;
        while (Module32NextW(snap, &me))
            if (_wcsicmp(me.szModule, mods[m]) == 0) { found = 1; break; }
        CloseHandle(snap);
        if (!found) { printf("%-16s not loaded\n", mods[m]); continue; }

        wchar_t path[MAX_PATH];
        swprintf(path, MAX_PATH, L"%s", me.szExePath);
        HANDLE f = CreateFileW(path, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
        if (f == INVALID_HANDLE_VALUE) { printf("%-16s file open failed\n", mods[m]); continue; }
        DWORD fsz = GetFileSize(f, NULL);
        unsigned char *fb = (unsigned char *)malloc(fsz);
        DWORD rd = 0;
        ReadFile(f, fb, fsz, &rd, NULL);
        CloseHandle(f);
        if (rd != fsz) { printf("%-16s read failed\n", mods[m]); free(fb); continue; }

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
        if (!text_size || text_size > raw_size) { printf("%-16s no .text\n", mods[m]); free(fb); continue; }

        nfix = 0;
        parse_relocs(fb, fsz, pe, ns, so);

        unsigned char *tb = (unsigned char *)malloc(text_size);
        SIZE_T got = 0;
        BOOL ok = ReadProcessMemory(h, (LPCVOID)((ULONG_PTR)me.modBaseAddr + text_rva), tb, text_size, &got);
        if (!ok || got != text_size) { printf("%-16s read failed err=%lu\n", mods[m], GetLastError()); free(fb); free(tb); continue; }

        int ndiff = 0, nshown = 0;
        for (DWORD i = 0; i < text_size; i++) {
            if (tb[i] == fb[raw_off + i]) continue;
            if (is_fixup(text_rva + i)) continue;
            if (nshown < 8) {
                printf("%-16s .text+0x%06x (rva 0x%06x): target=", mods[m], i, text_rva + i);
                for (int k = 0; k < 16 && (i + k) < text_size; k++) printf("%02x ", tb[i + k]);
                printf(" file=");
                for (int k = 0; k < 16 && (i + k) < text_size; k++) printf("%02x ", fb[raw_off + i + k]);
                printf("\n");
                nshown++;
            }
            ndiff++;
        }
        printf("%-16s .text=0x%08x fixups=%d REAL_DIFFS=%d\n", mods[m], text_size, nfix, ndiff);
        free(fb);
        free(tb);
    }
    CloseHandle(h);
    return 0;
}