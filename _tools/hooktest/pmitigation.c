/* Probe runtime mitigation policy via NtQueryInformationProcess (class 34).
 * Usage: pmitigation <pid>
 */
#include <windows.h>
#include <stdio.h>

typedef LONG NTSTATUS;
typedef NTSTATUS(NTAPI *NtQIP_t)(HANDLE, LONG, PVOID, ULONG, PULONG);

typedef struct {
    union {
        struct { UCHAR Level; UCHAR Type; UCHAR Audit; UCHAR Signer; };
        ULONG Flags;
    };
} PS_PROTECTION;

static void q_mit(NtQIP_t f, HANDLE h, LONG sub, const char *name) {
    ULONG buf[8] = { 0 };
    ULONG ret = 0;
    buf[0] = (ULONG)sub;
    NTSTATUS st = f(h, 34, buf, 4, &ret);
    printf("%-22s sub=%ld status=0x%08lx ret=%lu f0=0x%08lx f1=0x%08lx\n",
           name, sub, (ULONG)st, ret, buf[0], buf[1]);
}

int main(int argc, char **argv) {
    if (argc < 2) { printf("usage: pmitigation <pid>\n"); return 2; }
    DWORD pid = (DWORD)strtoul(argv[1], NULL, 10);
    HMODULE ntdll = LoadLibraryA("ntdll.dll");
    NtQIP_t f = (NtQIP_t)GetProcAddress(ntdll, "NtQueryInformationProcess");
    HANDLE h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
    if (!h) { printf("OpenProcess failed err=%lu\n", GetLastError()); return 1; }

    ULONG64 wow = 0; ULONG ret = 0;
    NTSTATUS st = f(h, 26, &wow, 8, &ret);
    printf("Wow64 status=0x%08lx wow64=%llu\n", (ULONG)st, (unsigned long long)wow);

    PS_PROTECTION prot; memset(&prot, 0, sizeof(prot));
    st = f(h, 61, &prot, sizeof(prot), &ret);
    printf("Protection status=0x%08lx type=%u signer=%u\n", (ULONG)st, prot.Type, prot.Signer);

    q_mit(f, h, 2,  "DynamicCode");
    q_mit(f, h, 6,  "ExtPointDisable");
    q_mit(f, h, 8,  "BinarySignature");
    q_mit(f, h, 10, "ImageLoad");
    q_mit(f, h, 13, "ChildProcess");

    CloseHandle(h);
    return 0;
}