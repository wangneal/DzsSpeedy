/* peexports.c — dump export names of a PE file (both 32/64-bit).
 * Usage: peexports <file>
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

static uint32_t rd32(const uint8_t *p) { return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24); }
static uint16_t rd16(const uint8_t *p) { return (uint16_t)(p[0] | ((uint16_t)p[1] << 8)); }

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: peexports <file>\n"); return 2; }
    FILE *f = fopen(argv[1], "rb");
    if (!f) { fprintf(stderr, "cannot open %s\n", argv[1]); return 1; }
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);
    uint8_t *buf = malloc(size);
    if (!buf || fread(buf, 1, size, f) != (size_t)size) { fprintf(stderr, "read failed\n"); return 1; }
    fclose(f);

    if (rd16(buf) != 0x5a4d) { fprintf(stderr, "not a PE\n"); return 1; }
    uint32_t pe = rd32(buf + 0x3c);
    if (pe + 24 + 224 > (uint32_t)size || rd32(buf + pe) != 0x00004550) { fprintf(stderr, "bad PE header\n"); return 1; }
    uint16_t magic = rd16(buf + pe + 24);
    uint32_t dd_off = pe + 24 + (magic == 0x10b ? 96 : 112);
    uint32_t export_rva = rd32(buf + dd_off);
    uint32_t export_size = rd32(buf + dd_off + 4);
    if (!export_rva) { printf("(no exports)\n"); return 0; }
    uint16_t nsec = rd16(buf + pe + 6);
    uint16_t optsize = rd16(buf + pe + 20);
    uint32_t sec_off = pe + 24 + optsize;
    (void)nsec;
    /* sections: name(8) vsize(4) vaddr(4) rsize(4) roff(4) ... */
    struct { uint32_t va, vsize, roff, rsize; } secs[16];
    int n = 0;
    for (uint32_t s = 0; s < nsec && n < 16; s++) {
        uint32_t o = sec_off + s * 40;
        secs[n].vsize = rd32(buf + o + 8);
        secs[n].va = rd32(buf + o + 12);
        secs[n].rsize = rd32(buf + o + 16);
        secs[n].roff = rd32(buf + o + 20);
        n++;
    }
    /* RVA->offset */
    uint32_t export_off = 0;
    for (int i = 0; i < n; i++) {
        uint32_t span = secs[i].vsize > secs[i].rsize ? secs[i].vsize : secs[i].rsize;
        if (export_rva >= secs[i].va && export_rva < secs[i].va + span) {
            export_off = secs[i].roff + (export_rva - secs[i].va);
            break;
        }
    }
    if (!export_off) { fprintf(stderr, "export dir not in sections\n"); return 1; }
    uint32_t name_rva = rd32(buf + export_off + 12);
    uint32_t base = rd32(buf + export_off + 16);
    uint32_t nfunc = rd32(buf + export_off + 20);
    uint32_t nnames = rd32(buf + export_off + 24);
    uint32_t addr_funcs_rva_unused = rd32(buf + export_off + 28);
    uint32_t names_rva = rd32(buf + export_off + 32);
    uint32_t ords_rva = rd32(buf + export_off + 36);
    /* dll name */
    for (int i = 0; i < n; i++) {
        uint32_t span = secs[i].vsize > secs[i].rsize ? secs[i].vsize : secs[i].rsize;
        if (name_rva >= secs[i].va && name_rva < secs[i].va + span) {
            printf("dll: %s\n", (char *)(buf + secs[i].roff + (name_rva - secs[i].va)));
            break;
        }
    }
    printf("base=%u functions=%u names=%u\n", base, nfunc, nnames);
    /* dump function RVAs in ordinal order (first 32) */
    for (uint32_t i = 0; i < nfunc && i < 32; i++) {
        uint32_t frva = 0;
        for (int k = 0; k < n; k++) {
            uint32_t span = secs[k].vsize > secs[k].rsize ? secs[k].vsize : secs[k].rsize;
            if (addr_funcs_rva_unused + i * 4 >= secs[k].va && addr_funcs_rva_unused + i * 4 < secs[k].va + span) {
                frva = rd32(buf + secs[k].roff + (addr_funcs_rva_unused + i * 4 - secs[k].va));
                break;
            }
        }
        printf("  func[%u] (ordinal %u) rva=0x%08X\n", i, base + i, frva);
    }
    for (uint32_t i = 0; i < nnames; i++) {
        uint32_t nrva = 0, ord = 0;
        for (int k = 0; k < n; k++) {
            uint32_t span = secs[k].vsize > secs[k].rsize ? secs[k].vsize : secs[k].rsize;
            if (names_rva + i * 4 >= secs[k].va && names_rva + i * 4 < secs[k].va + span) {
                nrva = rd32(buf + secs[k].roff + (names_rva + i * 4 - secs[k].va));
                break;
            }
        }
        for (int k = 0; k < n; k++) {
            uint32_t span = secs[k].vsize > secs[k].rsize ? secs[k].vsize : secs[k].rsize;
            if (ords_rva + i * 2 >= secs[k].va && ords_rva + i * 2 < secs[k].va + span) {
                ord = rd16(buf + secs[k].roff + (ords_rva + i * 2 - secs[k].va)) + base;
                break;
            }
        }
        for (int k = 0; k < n; k++) {
            uint32_t span = secs[k].vsize > secs[k].rsize ? secs[k].vsize : secs[k].rsize;
            if (nrva >= secs[k].va && nrva < secs[k].va + span) {
                printf("  ordinal %u: %s\n", ord, (char *)(buf + secs[k].roff + (nrva - secs[k].va)));
                break;
            }
        }
    }
    free(buf);
    return 0;
}