/* Manual-mapped (loader-free) DLL injection for 32-bit targets.
 * Resolves imports against already-loaded modules (same bases in shared images),
 * applies relocations, then calls DllMain via CreateRemoteThread.
 * Usage: remmap <pid> <dllpath>
 */
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <stdint.h>

static DWORD rva2off(unsigned char *fb, DWORD pe, DWORD so, WORD ns, DWORD rva);

static void *read_file(const char *path, DWORD *size) {
    HANDLE f = CreateFileA(path, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
    if (f == INVALID_HANDLE_VALUE) return NULL;
    DWORD sz = GetFileSize(f, NULL);
    void *buf = malloc(sz);
    DWORD rd = 0;
    ReadFile(f, buf, sz, &rd, NULL);
    CloseHandle(f);
    if (rd != sz) { free(buf); return NULL; }
    *size = sz;
    return buf;
}

int main(int argc, char **argv) {
    if (argc < 3) { printf("usage: remmap <pid> <dllpath>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 10);

    DWORD fsz = 0;
    unsigned char *fb = read_file(argv[2], &fsz);
    if (!fb) { printf("read file failed\n"); return 1; }
    if (*(WORD *)(fb) != 0x5a4d) { printf("not PE\n"); return 1; }
    DWORD pe = *(DWORD *)(fb + 0x3c);
    DWORD opt = pe + 24;
    WORD mg = *(WORD *)(fb + opt);
    if (mg != 0x10b) { printf("not PE32 (magic=%x)\n", mg); return 1; }
    DWORD imagebase = *(DWORD *)(fb + opt + 28);
    DWORD size_image = *(DWORD *)(fb + opt + 56);
    WORD ns = *(WORD *)(fb + pe + 6);
    DWORD so = opt + 224;
    DWORD ep_rva = *(DWORD *)(fb + opt + 16);
    DWORD reloc_rva = 0, reloc_size = 0;
    DWORD imp_rva = 0, imp_size = 0;

    printf("imagebase=0x%08x size=0x%08x ep=0x%08x ns=%u\n", imagebase, size_image, ep_rva, ns);

    /* find .reloc and import dir from data directories (dir 1=import, 5=reloc, 14=delay) */
    imp_rva = *(DWORD *)(fb + opt + 104);
    imp_size = *(DWORD *)(fb + opt + 108);
    reloc_rva = *(DWORD *)(fb + opt + 160);
    reloc_size = *(DWORD *)(fb + opt + 164);
    printf("imp rva=0x%08x sz=0x%08x; reloc rva=0x%08x sz=0x%08x\n", imp_rva, imp_size, reloc_rva, reloc_size);

    HANDLE h = OpenProcess(PROCESS_ALL_ACCESS, FALSE, pid);
    if (!h) { printf("OpenProcess failed err=%lu\n", GetLastError()); return 1; }

    /* allocate image in target: prefer imagebase (no relocs), else any base */
    void *base = VirtualAllocEx(h, (LPVOID)(uintptr_t)imagebase, size_image, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
    if (!base && reloc_rva != 0) {
        printf("preferred base 0x%08x busy; gap scan fallback (relocs available)\n", imagebase);
        /* find a free gap among loaded modules */
        PIMAGE_DOS_HEADER dos = (PIMAGE_DOS_HEADER)fb;
        (void)dos;
        HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
        if (snap != INVALID_HANDLE_VALUE) {
            MODULEENTRY32W me; me.dwSize = sizeof(me);
            DWORD lo = 0x10000, hi = 0x7fff0000;
            while (Module32NextW(snap, &me)) {
                DWORD mb = (DWORD)(uintptr_t)me.modBaseAddr;
                DWORD ms = me.modBaseSize;
                if (mb >= lo && mb < hi && mb + ms > lo && mb < lo + size_image) {
                    lo = mb + ms + 0x1000;
                    lo = (lo + 0xffff) & ~0xffffu;
                }
            }
            CloseHandle(snap);
            if (lo + size_image < hi) {
                base = VirtualAllocEx(h, (LPVOID)(uintptr_t)lo, size_image, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
                printf("gap alloc at 0x%08x: %s\n", lo, base ? "OK" : "FAILED");
            }
        }
    }
    if (!base) { printf("VirtualAllocEx failed err=%lu\n", GetLastError()); return 1; }
    printf("allocated at %p\n", base);

    /* write headers */
    if (!WriteProcessMemory(h, base, fb, opt + 224 + ns * 40 > fsz ? fsz : opt + 224 + ns * 40, NULL)) {
        printf("WPM headers failed err=%lu\n", GetLastError()); return 1;
    }
    /* write sections (raw) */
    for (int i = 0; i < ns; i++) {
        DWORD s = so + i * 40;
        DWORD vsz = *(DWORD *)(fb + s + 8);
        DWORD vaddr = *(DWORD *)(fb + s + 12);
        DWORD rsz = *(DWORD *)(fb + s + 16);
        DWORD roff = *(DWORD *)(fb + s + 20);
        if (roff && rsz) {
            if (!WriteProcessMemory(h, (char *)base + vaddr, fb + roff, rsz, NULL)) {
                printf("WPM section %d failed err=%lu\n", i, GetLastError());
            }
        }
        (void)vsz;
    }

    /* relocations */
    uint32_t delta = (uint32_t)(uintptr_t)base - imagebase;
    if (delta && reloc_rva && reloc_size) {
        DWORD roff = rva2off(fb, pe, so, ns, reloc_rva);
        if (roff) {
            DWORD end = roff + reloc_size;
            DWORD off = roff;
            int nfix = 0;
            while (off + 8 <= end && off + 8 <= fsz) {
                DWORD page = *(DWORD *)(fb + off);
                DWORD cnt = *(DWORD *)(fb + off + 4);
                if (!cnt) break;
                for (DWORD e = off + 8; e + 2 <= off + cnt && e + 2 <= end && e + 2 <= fsz; e += 2) {
                    WORD ent = *(WORD *)(fb + e);
                    WORD type = ent >> 12;
                    WORD rel = ent & 0xfff;
                    if (type == 3) {
                        uint32_t addr = (uint32_t)(uintptr_t)base + page + rel;
                        uint32_t val = 0;
                        SIZE_T got = 0;
                        ReadProcessMemory(h, (void *)addr, &val, 4, &got);
                        val += delta;
                        WriteProcessMemory(h, (void *)addr, &val, 4, NULL);
                        nfix++;
                    }
                }
                off += cnt;
            }
            printf("relocs applied=%d (delta=0x%08x)\n", nfix, delta);
        }
    } else {
        printf("no relocs needed (delta=0x%08x)\n", delta);
    }

    /* imports: resolve from LOCAL loaded modules (shared bases => same addresses in target) */
    if (imp_rva) {
        DWORD ioff = rva2off(fb, pe, so, ns, imp_rva);
        int ndesc = 0;
        while (ioff + 20 <= fsz) {
            DWORD oft = *(DWORD *)(fb + ioff);
            DWORD name_rva = *(DWORD *)(fb + ioff + 12);
            DWORD thunk_rva = *(DWORD *)(fb + ioff + 16);
            if (!name_rva) break;
            DWORD noff = rva2off(fb, pe, so, ns, name_rva);
            char dllname[128];
            strncpy(dllname, (char *)fb + noff, 127);
            dllname[127] = 0;
            HMODULE lm = GetModuleHandleA(dllname);
            if (!lm) lm = LoadLibraryA(dllname);   /* api-set names resolve locally */
            if (!lm) { printf("import dll NOT loadable locally: %s\n", dllname); return 1; }
            DWORD taddr = (DWORD)(uintptr_t)base + thunk_rva;
            DWORD tsrc = oft ? oft : thunk_rva;
            int nimp = 0;
            for (int k = 0; ; k++) {
                DWORD entry = *(DWORD *)(fb + rva2off(fb, pe, so, ns, tsrc) + k * 4);
                if (!entry) break;
                FARPROC p;
                if (entry & 0x80000000) p = (FARPROC)GetProcAddress(lm, (LPCSTR)(entry & 0xffff));
                else p = GetProcAddress(lm, (LPCSTR)(fb + rva2off(fb, pe, so, ns, entry)));
                if (!p) { printf("import not resolved: %s!ord/name\n", dllname); return 1; }
                WriteProcessMemory(h, (void *)(taddr + k * 4), &p, 4, NULL);
                nimp++;
            }
            printf("imports: %s -> %d thunks (module %p)\n", dllname, nimp, lm);
            ioff += 20;
            ndesc++;
        }
        (void)ndesc;
    }

    /* call DllMain(DLL_PROCESS_ATTACH) */
    FARPROC ep = (FARPROC)((char *)base + ep_rva);
    HANDLE t = CreateRemoteThread(h, NULL, 0, (LPTHREAD_START_ROUTINE)ep, NULL, 0, NULL);
    if (!t) { printf("CreateRemoteThread DllMain failed err=%lu\n", GetLastError()); return 1; }
    DWORD w = WaitForSingleObject(t, 15000);
    DWORD exitc = 0;
    GetExitCodeThread(t, &exitc);
    printf("DllMain thread wait=%lu exit=0x%08lx\n", w, exitc);
    fflush(stdout);
    CloseHandle(t);
    printf("DONE (module remains mapped at %p)\n", base);
    return 0;
}

/* forward decls used above */
static DWORD rva2off(unsigned char *fb, DWORD pe, DWORD so, WORD ns, DWORD rva) {
    for (int i = 0; i < ns; i++) {
        DWORD s = so + i * 40;
        DWORD vaddr = *(DWORD *)(fb + s + 12);
        DWORD vsz = *(DWORD *)(fb + s + 8);
        DWORD roff = *(DWORD *)(fb + s + 20);
        if (rva >= vaddr && rva < vaddr + vsz) return roff + (rva - vaddr);
    }
    return 0;
}