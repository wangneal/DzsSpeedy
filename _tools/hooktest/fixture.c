/* hooktest fixture: GUI window + message loop + an extra pumping worker thread.
 * Mirrors a game process: several threads, at least one with a real message pump.
 */
#include <windows.h>
#include <stdio.h>

static void report_tid(const char *role) {
    char tmp[MAX_PATH];
    if (!GetTempPathA(MAX_PATH, tmp)) return;
    char path[MAX_PATH];
    snprintf(path, sizeof(path), "%shooktest-tid-%lu.txt", tmp, GetCurrentProcessId());
    FILE *f = fopen(path, "a");
    if (f) {
        fprintf(f, "%s tid=%lu\n", role, GetCurrentThreadId());
        fclose(f);
    }
}

static LRESULT CALLBACK WndProc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    default:
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
}

static DWORD WINAPI pump_thread(LPVOID arg) {
    report_tid("worker");
    MSG msg;
    while (GetMessageW(&msg, NULL, 0, 0) > 0) {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    return 0;
}

int main(void) {
    /* extra worker thread that also pumps messages */
    CreateThread(NULL, 0, pump_thread, NULL, 0, NULL);

    WNDCLASSW wc;
    memset(&wc, 0, sizeof(wc));
    wc.lpfnWndProc = WndProc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"HookTestFixtureWnd";
    wc.hCursor = LoadCursorW(NULL, IDC_ARROW);
    if (!RegisterClassW(&wc)) {
        fprintf(stderr, "RegisterClassW failed gle=%lu\n", GetLastError());
        return 1;
    }
    HWND hwnd = CreateWindowExW(0, L"HookTestFixtureWnd", L"hooktest-fixture",
                                WS_OVERLAPPEDWINDOW, CW_USEDEFAULT, CW_USEDEFAULT,
                                400, 300, NULL, NULL, wc.hInstance, NULL);
    if (!hwnd) {
        fprintf(stderr, "CreateWindowExW failed gle=%lu\n", GetLastError());
        return 2;
    }
    ShowWindow(hwnd, SW_SHOW);
    UpdateWindow(hwnd);
    report_tid("main");

    MSG msg;
    while (GetMessageW(&msg, NULL, 0, 0) > 0) {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    return (int)msg.wParam;
}
