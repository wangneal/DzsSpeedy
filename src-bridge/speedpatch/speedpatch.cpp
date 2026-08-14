/*
 * DzsSpeedy (斗战神游戏加速器) — Windows 时间感知加速控制器
 * Copyright (C) 2026 wangneal
 *
 * This program is free software: you can redistribute it
 * and/or modify it under the terms of the GNU General
 * Public License as published by the Free Software
 * Foundation, either version 3 of the License, or (at your
 * option) any later version.
 *
 * This program is distributed in the hope that it will be
 * useful, but WITHOUT ANY WARRANTY; without even the
 * implied warranty of MERCHANTABILITY or FITNESS FOR A
 * PARTICULAR PURPOSE.  See the GNU General Public License
 * for more details.
 *
 * You should have received a copy of the GNU General Public
 * License along with this program.  If not, see
 * <https://www.gnu.org/licenses/>.
 */
#include <windows.h>
#include <winternl.h>
#include "Minhook.h"
#include "speedpatch.h"
#include <atomic>
#include <mmsystem.h>
#include <shared_mutex>
#include <sstream>
#pragma comment(lib, "winmm.lib")
#pragma data_seg("shared")
static volatile double factor = 1.0;
#pragma data_seg()
#pragma comment(linker, "/section:shared,RWS")

static std::shared_mutex mutex;
static std::atomic<double> pre_factor = 1.0;
static std::atomic<LONG> initState = 0;
static HANDLE hFileMap;
static LONG* pState;
static DWORD installErrorCode;

static constexpr LONG SP_STATE_INITIALIZING = 0x49;
static constexpr LONG SP_STATE_DISABLED = 0x44;
static constexpr LONG SP_STATE_ENABLED = 0x45;
static constexpr LONG SP_STATE_FAILED = 0x46;

static LONG SP_ReadState(volatile LONG* state)
{
    return InterlockedCompareExchange(state, 0, 0);
}

static void SP_WriteState(volatile LONG* state, LONG value)
{
    InterlockedExchange(state, value);
}

static bool SP_CompareAndSetState(volatile LONG* state, LONG expected, LONG value)
{
    return InterlockedCompareExchange(state, value, expected) == expected;
}

static constexpr DWORD SP_INIT_MINHOOK_ERROR = 0x01000000;
static constexpr DWORD SP_INIT_HOOK_CREATE_ERROR = 0x02000000;
static constexpr DWORD SP_INIT_HOOK_ENABLE_ERROR = 0x03000000;
static constexpr DWORD SP_INIT_MAPPING_CREATE_ERROR = 0x04000000;
static constexpr DWORD SP_INIT_MAPPING_VIEW_ERROR = 0x05000000;
static constexpr DWORD SP_INIT_ROLLBACK_ERROR = 0x06000000;
static constexpr DWORD SP_INIT_RESTART_REQUIRED = 0x07000000;

static DWORD SP_EncodeMhError(DWORD kind, MH_STATUS status, DWORD hookId = 0)
{
    return kind | ((hookId & 0xff) << 16) |
           (static_cast<DWORD>(status) & 0xffff);
}

// ── 诊断日志 ──────────────────────────────────────────────────────────────
// 写入 DebugView (https://learn.microsoft.com/sysinternals/downloads/debugview)
// 必须在 Release 构建时也能输出，因此用 OutputDebugStringW 而不是 OutputDebugStringA
// 用前缀 [DzsSpeedy] 方便过滤
static void SP_DbgLog(const wchar_t* fmt, ...)
{
    wchar_t buf[1024];
    wchar_t prefix[] = L"[DzsSpeedy][pid=";
    // 把 pid 与 msg 拼到 buf
    int n = 0;
    n += wsprintfW(buf, L"%s%lu]", prefix, GetCurrentProcessId());
    va_list ap;
    va_start(ap, fmt);
    n += _vsnwprintf_s(buf + n, _countof(buf) - n, _TRUNCATE, fmt, ap);
    va_end(ap);
    // 同时输出到 OutputDebugString 与文件（写文件跨进程也能取证）
    OutputDebugStringW(buf);

    // 写文件: %TEMP%\dzsspeedy-speedpatch-<pid>.log
    wchar_t tmpPath[MAX_PATH];
    DWORD len = GetTempPathW(_countof(tmpPath), tmpPath);
    if (len > 0 && len < _countof(tmpPath))
    {
        wchar_t filePath[MAX_PATH];
        _snwprintf_s(filePath, _countof(filePath), _TRUNCATE,
                     L"%sdzsspeedy-speedpatch-%lu.log", tmpPath, GetCurrentProcessId());
        HANDLE hf = CreateFileW(filePath, FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE,
                                nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
        if (hf != INVALID_HANDLE_VALUE)
        {
            DWORD wrote = 0;
            // 补换行（snwprintf 不会自动加）
            if (n < (int)_countof(buf) - 1) { buf[n++] = L'\n'; buf[n] = L'\0'; }
            WriteFile(hf, buf, (DWORD)(n * sizeof(wchar_t)), &wrote, nullptr);
            CloseHandle(hf);
        }
    }
}

static const wchar_t* SP_MhStatusName(MH_STATUS s)
{
    switch (s)
    {
        case MH_OK:                       return L"MH_OK";
        case MH_ERROR_ALREADY_INITIALIZED:return L"MH_ERROR_ALREADY_INITIALIZED";
        case MH_ERROR_NOT_INITIALIZED:    return L"MH_ERROR_NOT_INITIALIZED";
        case MH_ERROR_ALREADY_CREATED:    return L"MH_ERROR_ALREADY_CREATED";
        case MH_ERROR_NOT_CREATED:        return L"MH_ERROR_NOT_CREATED";
        case MH_ERROR_ENABLED:            return L"MH_ERROR_ENABLED";
        case MH_ERROR_DISABLED:           return L"MH_ERROR_DISABLED";
        case MH_ERROR_NOT_EXECUTABLE:     return L"MH_ERROR_NOT_EXECUTABLE";
        case MH_ERROR_UNSUPPORTED_FUNCTION:return L"MH_ERROR_UNSUPPORTED_FUNCTION";
        case MH_ERROR_MEMORY_ALLOC:       return L"MH_ERROR_MEMORY_ALLOC";
        case MH_ERROR_FUNCTION_NOT_FOUND: return L"MH_ERROR_FUNCTION_NOT_FOUND";
        default:                          return L"MH_STATUS_UNKNOWN";
    }
}

typedef VOID (WINAPI* SLEEP) (DWORD);
typedef DWORD (WINAPI* SLEEPEX) (DWORD, BOOL);

typedef UINT_PTR (WINAPI* SETTIMER) (
    HWND,
    UINT_PTR,
    UINT,
    TIMERPROC
    );
typedef DWORD (WINAPI* TIMEGETTIME) (VOID);
typedef MMRESULT (WINAPI* TIMESETEVENT) (
    UINT,
    UINT,
    LPTIMECALLBACK,
    DWORD_PTR,
    UINT
    );

typedef LONG (WINAPI* GETMESSAGETIME) (VOID);
typedef DWORD (WINAPI* GETTICKCOUNT) (VOID);
typedef ULONGLONG (WINAPI* GETTICKCOUNT64) (VOID);

typedef BOOL (WINAPI* QUERYPERFORMANCECOUNTER) (LARGE_INTEGER*);
typedef BOOL (WINAPI* QUERYPERFORMANCEFREQUENCY) (LARGE_INTEGER*);

typedef VOID (WINAPI* GETSYSTEMTIMEASFILETIME) (LPFILETIME);
typedef VOID (WINAPI* GETSYSTEMTIMEPRECISEASFILETIME) (LPFILETIME);

typedef BOOL (WINAPI* SETWAITABLETIMER) (
    HANDLE,
    const LARGE_INTEGER*,
    LONG,
    PTIMERAPCROUTINE,
    LPVOID,
    BOOL);

typedef BOOL (WINAPI* SETWAITABLETIMEREX) (
    HANDLE,
    const LARGE_INTEGER*,
    LONG,
    PTIMERAPCROUTINE,
    LPVOID,
    PREASON_CONTEXT,
    ULONG);

inline VOID shouldUpdateAll();

static SLEEP realSleep = NULL;

static SLEEPEX realSleepEx = NULL;

static SETTIMER realSetTimer = NULL;

static TIMEGETTIME realTimeGetTime = NULL;

static TIMESETEVENT realTimeSetEvent = NULL;

static GETMESSAGETIME realGetMessageTime = NULL;

static GETTICKCOUNT realGetTickCount = NULL;

static GETTICKCOUNT64 realGetTickCount64 = NULL;

static QUERYPERFORMANCECOUNTER realQueryPerformanceCounter = NULL;

static QUERYPERFORMANCEFREQUENCY realQueryPerformanceFrequency = NULL;

static GETSYSTEMTIMEASFILETIME realGetSystemTimeAsFileTime = NULL;

static GETSYSTEMTIMEPRECISEASFILETIME realGetSystemTimePreciseAsFileTime = NULL;

static SETWAITABLETIMER realSetWaitableTimer = NULL;

static SETWAITABLETIMEREX realSetWaitableTimerEx = NULL;

SPEEDPATCH_API void SP_SetSpeed(double factor_)
{
    factor = factor_;
}

SPEEDPATCH_API double SP_GetSpeed()
{
    return factor;
}

void SP_Install()
{
    installErrorCode = ERROR_SUCCESS;
    DWORD processId = GetCurrentProcessId();
    std::wstring filemapName = GetProcessFileMapName(processId);
    SP_DbgLog(L"SP_Install: enter, filemap=%s", filemapName.c_str());

    SECURITY_DESCRIPTOR sd;
    if (!InitializeSecurityDescriptor(&sd, SECURITY_DESCRIPTOR_REVISION) ||
        !SetSecurityDescriptorDacl(&sd, TRUE, NULL, FALSE))
    {
        DWORD err = GetLastError();
        installErrorCode = SP_INIT_MAPPING_CREATE_ERROR | (err & 0x00ffffff);
        SP_DbgLog(L"SP_Install: security descriptor setup FAILED err=%lu (0x%08lx)",
                  err, err);
        return;
    }
    SECURITY_ATTRIBUTES sa;
    sa.nLength = sizeof(sa);
    sa.lpSecurityDescriptor = &sd;
    sa.bInheritHandle = FALSE;

    hFileMap = CreateFileMapping(
        INVALID_HANDLE_VALUE,
        &sa,
        PAGE_READWRITE,
        0,
        sizeof(LONG),
        filemapName.c_str()
        );
    if (hFileMap == NULL)
    {
        DWORD err = GetLastError();
        installErrorCode = SP_INIT_MAPPING_CREATE_ERROR | (err & 0x00ffffff);
        SP_DbgLog(L"SP_Install: CreateFileMapping FAILED err=%lu (0x%08lx) name=%s",
                  err, err, filemapName.c_str());
        return;
    }
    pState = static_cast<LONG*>(MapViewOfFile(
        hFileMap,
        FILE_MAP_ALL_ACCESS,
        0,
        0,
        sizeof(LONG)
        ));
    if (pState == NULL)
    {
        DWORD err = GetLastError();
        installErrorCode = SP_INIT_MAPPING_VIEW_ERROR | (err & 0x00ffffff);
        SP_DbgLog(L"SP_Install: MapViewOfFile FAILED err=%lu (0x%08lx)", err, err);
        CloseHandle(hFileMap);
        hFileMap = NULL;
        return;
    }
    SP_WriteState(pState, SP_STATE_INITIALIZING);
    SP_DbgLog(L"SP_Install: OK, hFileMap=%p pState=%p", hFileMap, (void*)pState);
}

void SP_Uninstall()
{
    if (hFileMap != NULL)
    {
        if (pState != nullptr)
        {
            UnmapViewOfFile(pState);
        }
        CloseHandle(hFileMap);
        pState = nullptr;
        hFileMap = NULL;
    }
}
SPEEDPATCH_API DWORD WINAPI SP_Shutdown(LPVOID)
{
    if (pState != nullptr && SP_ReadState(pState) != SP_STATE_FAILED)
    {
        SP_WriteState(pState, SP_STATE_DISABLED);
    }
    SP_DbgLog(L"SP_Shutdown: live unload refused; hooks disabled logically only");
    return ERROR_NOT_SUPPORTED;
}

BOOL SP_IsEnabled()
{
    return pState != nullptr && SP_ReadState(pState) == SP_STATE_ENABLED;
}

SPEEDPATCH_API BOOL SP_IsEnabledById(DWORD processId)
{
    std::wstring filemapName = GetProcessFileMapName(processId);
    HANDLE hFileMap_ = OpenFileMapping(FILE_MAP_READ,
                                     FALSE,
                                     filemapName.c_str()
                                     );
    if (hFileMap_ == NULL)
    {
        return FALSE;
    }
    LONG* pStatus = static_cast<LONG*>(MapViewOfFile(hFileMap_,
                                                     FILE_MAP_READ,
                                                     0,
                                                     0,
                                                     sizeof(LONG)));
    if (pStatus == NULL)
    {
        CloseHandle(hFileMap_);
        return FALSE;
    }
    BOOL enabled = SP_ReadState(pStatus) == SP_STATE_ENABLED;
    UnmapViewOfFile(pStatus);
    CloseHandle(hFileMap_);
    return enabled;
}

void SP_Enable(DWORD processId)
{
    std::wstring filemapName = GetProcessFileMapName(processId);
    HANDLE hFileMap_ = OpenFileMapping(FILE_MAP_ALL_ACCESS,
                                     FALSE,
                                     filemapName.c_str()
                                     );
    if (hFileMap_ == NULL)
    {
        return;
    }
    LONG* pStatus = static_cast<LONG*>(MapViewOfFile(hFileMap_,
                                                     FILE_MAP_ALL_ACCESS,
                                                     0,
                                                     0,
                                                     sizeof(LONG)));
    if (pStatus == NULL)
    {
        CloseHandle(hFileMap_);
        return;
    }
    if (SP_ReadState(pStatus) != SP_STATE_FAILED)
    {
        SP_WriteState(pStatus, SP_STATE_ENABLED);
    }
    UnmapViewOfFile(pStatus);
    CloseHandle(hFileMap_);
}

void SP_Disable(DWORD processId)
{
    std::wstring filemapName = GetProcessFileMapName(processId);
    HANDLE hFileMap_ = OpenFileMapping(FILE_MAP_ALL_ACCESS,
                                     FALSE,
                                     filemapName.c_str()
                                     );
    if (hFileMap_ == NULL)
    {
        return;
    }
    LONG* pStatus = static_cast<LONG*>(MapViewOfFile(hFileMap_,
                                                     FILE_MAP_ALL_ACCESS,
                                                     0,
                                                     0,
                                                     sizeof(LONG)));
    if (pStatus == NULL)
    {
        CloseHandle(hFileMap_);
        return;
    }
    if (SP_ReadState(pStatus) != SP_STATE_FAILED)
    {
        SP_WriteState(pStatus, SP_STATE_DISABLED);
    }
    UnmapViewOfFile(pStatus);
    CloseHandle(hFileMap_);
}

std::wstring GetCurrentProcessName()
{
    wchar_t processPath[MAX_PATH];
    GetModuleFileName(NULL, processPath, MAX_PATH);
    std::wstring fullPath(processPath);
    size_t lastSlash = fullPath.find_last_of(L"\\");
    if (lastSlash != std::wstring::npos)
    {
        fullPath = fullPath.substr(lastSlash + 1);
    }
    return fullPath;
}

std::wstring GetProcessFileMapName(DWORD processId)
{
    std::wstringstream wss;
    wss << L"DzsSpeedy." << processId;
    return wss.str();
}

double SpeedFactor()
{
    if (SP_IsEnabled())
    {
        return factor;
    }
    else
    {
        return 1.0;
    }
}

VOID WINAPI DetourSleep(DWORD dwMilliseconds)
{
    std::shared_lock<std::shared_mutex> lock(mutex);
    realSleep(dwMilliseconds / SpeedFactor());
}

DWORD WINAPI DetourSleepEx(DWORD dwMilliseconds, BOOL bAlertable)
{
    std::shared_lock<std::shared_mutex> lock(mutex);
    return realSleepEx(dwMilliseconds / SpeedFactor(), bAlertable);
}

UINT_PTR WINAPI DetourSetTimer(HWND      hWnd,
                               UINT_PTR  nIDEvent,
                               UINT      uElapse,
                               TIMERPROC lpTimerFunc)
{
    std::shared_lock<std::shared_mutex> lock(mutex);
    return realSetTimer(
        hWnd,
        nIDEvent,
        uElapse / SpeedFactor(),
        lpTimerFunc
        );
}

static DWORD baselineKernelTimeGetTime = 0;
static DWORD baselineDetourTimeGetTime = 0;
static DWORD prevcallKernelTimeGetTime = 0;
static DWORD prevcallDetourTimeGetTime = 0;
static std::atomic<bool> shouldUpdateTimeGetTime = false;

DWORD WINAPI DetourTimeGetTime(VOID)
{
    std::shared_lock<std::shared_mutex> lock(mutex);
    if (pre_factor != SpeedFactor())
    {
        pre_factor = SpeedFactor();
        shouldUpdateAll();
    }
    bool expected = true;
    if (shouldUpdateTimeGetTime.compare_exchange_weak(expected, false))
    {
        baselineKernelTimeGetTime = prevcallKernelTimeGetTime;
        baselineDetourTimeGetTime = prevcallDetourTimeGetTime;
    }
    DWORD now = realTimeGetTime();
    prevcallKernelTimeGetTime = now;
    DWORD delta = SpeedFactor() * (now - baselineKernelTimeGetTime);
    prevcallDetourTimeGetTime = baselineDetourTimeGetTime + delta;
    return baselineDetourTimeGetTime + delta;
}

MMRESULT WINAPI DetourTimeSetEvent(UINT           uDelay,
                                   UINT           uResolution,
                                   LPTIMECALLBACK lpTimeProc,
                                   DWORD_PTR      dwUser,
                                   UINT           fuEvent)
{
    return realTimeSetEvent(
        uDelay / SpeedFactor(),
        uResolution,
        lpTimeProc,
        dwUser,
        fuEvent);
}

static LONG baselineKernelGetMessageTime = 0;
static LONG baselineDetourGetMessageTime = 0;
static LONG prevcallKernelGetMessageTime = 0;
static LONG prevcallDetourGetMessageTime = 0;
static std::atomic<bool> shouldUpdateGetMessageTime = false;
LONG WINAPI DetourGetMessageTime(VOID)
{
    std::shared_lock<std::shared_mutex> lock(mutex);
    if (pre_factor != SpeedFactor())
    {
        pre_factor = SpeedFactor();
        shouldUpdateAll();
    }
    bool expected = true;
    if (shouldUpdateGetMessageTime.compare_exchange_weak(expected, false))
    {
        baselineKernelGetMessageTime = prevcallKernelGetMessageTime;
        baselineDetourGetMessageTime = prevcallDetourGetMessageTime;
    }
    LONG now = realGetMessageTime();
    prevcallKernelGetMessageTime = now;
    DWORD delta = SpeedFactor() * (now - baselineKernelGetMessageTime);
    prevcallDetourGetMessageTime = baselineDetourGetMessageTime + delta;
    return baselineDetourGetMessageTime + delta;
}

static DWORD baselineKernelGetTickCount = 0;
static DWORD baselineDetourGetTickCount = 0;
static DWORD prevcallKernelGetTickCount = 0;
static DWORD prevcallDetourGetTickCount = 0;
static std::atomic<bool> shouldUpdateGetTickCount = false;
DWORD WINAPI DetourGetTickCount(VOID)
{
    std::shared_lock<std::shared_mutex> lock(mutex);
    if (pre_factor != SpeedFactor())
    {
        pre_factor = SpeedFactor();
        shouldUpdateAll();
    }
    bool expected = true;
    if (shouldUpdateGetTickCount.compare_exchange_weak(expected, false))
    {
        baselineKernelGetTickCount = prevcallKernelGetTickCount;
        baselineDetourGetTickCount = prevcallDetourGetTickCount;
    }
    DWORD now = realGetTickCount();
    prevcallKernelGetTickCount = now;
    DWORD delta = SpeedFactor() * (now - baselineKernelGetTickCount);
    prevcallDetourGetTickCount = baselineDetourGetTickCount + delta;
    return baselineDetourGetTickCount + delta;
}

static ULONGLONG baselineKernelGetTickCount64 = 0;
static ULONGLONG baselineDetourGetTickCount64 = 0;
static ULONGLONG prevcallKernelGetTickCount64 = 0;
static ULONGLONG prevcallDetourGetTickCount64 = 0;
std::atomic<bool> shouldUpdateGetTickCount64 = false;
ULONGLONG WINAPI DetourGetTickCount64(VOID)
{
    std::shared_lock<std::shared_mutex> lock(mutex);
    if (pre_factor != SpeedFactor())
    {
        pre_factor = SpeedFactor();
        shouldUpdateAll();
    }
    bool expected = true;
    if (shouldUpdateGetTickCount64.compare_exchange_weak(expected, false))
    {
        baselineKernelGetTickCount64 = prevcallKernelGetTickCount64;
        baselineDetourGetTickCount64 = prevcallDetourGetTickCount64;
    }
    ULONGLONG now = realGetTickCount64();
    prevcallKernelGetTickCount64 = now;
    ULONGLONG delta = SpeedFactor() * (now - baselineKernelGetTickCount64);
    prevcallDetourGetTickCount64 = baselineDetourGetTickCount64 + delta;
    return baselineDetourGetTickCount64 + delta;
}

static LARGE_INTEGER baselineKernelQueryPerformanceCounter = { 0 };
static LARGE_INTEGER baselineDetourQueryPerformanceCounter = { 0 };
static LARGE_INTEGER prevcallKernelQueryPerformanceCounter = { 0 };
static LARGE_INTEGER prevcallDetourQueryPerformanceCounter = { 0 };
static std::atomic<bool> shouldUpdateQueryPerformanceCounter = false;
BOOL WINAPI DetourQueryPerformanceCounter(LARGE_INTEGER* lpPerformanceCount)
{
    std::shared_lock<std::shared_mutex> lock(mutex);
    if (lpPerformanceCount == NULL)
    {
        return FALSE;
    }
    if (pre_factor != SpeedFactor())
    {
        pre_factor = SpeedFactor();
        shouldUpdateAll();
    }
    // 更新基准时间点
    bool expected = true;
    if (shouldUpdateQueryPerformanceCounter.compare_exchange_weak(expected,
                                                                  false))
    {
        baselineKernelQueryPerformanceCounter = prevcallKernelQueryPerformanceCounter;
        baselineDetourQueryPerformanceCounter = prevcallDetourQueryPerformanceCounter;
    }
    BOOL rtncode = realQueryPerformanceCounter(
        &prevcallKernelQueryPerformanceCounter);
    if (rtncode == TRUE)
    {
        *lpPerformanceCount = prevcallKernelQueryPerformanceCounter;
    }
    LONGLONG delta =
        SpeedFactor() * (lpPerformanceCount->QuadPart -
                         baselineKernelQueryPerformanceCounter.QuadPart)
    ;
    lpPerformanceCount->QuadPart = baselineDetourQueryPerformanceCounter.QuadPart + delta;
    prevcallDetourQueryPerformanceCounter = *lpPerformanceCount;
    return rtncode;
}

static LARGE_INTEGER baselineKernelQueryPerformanceFrequency = { 0 };
BOOL WINAPI DetourQueryPerformanceFrequency(LARGE_INTEGER* lpFrequency)
{
    std::shared_lock<std::shared_mutex> lock(mutex);
    if (lpFrequency == NULL)
    {
        return FALSE;
    }
    else
    {
        BOOL rtncode = realQueryPerformanceFrequency(lpFrequency);
        lpFrequency->QuadPart = SpeedFactor() * lpFrequency->QuadPart;
        return rtncode;
    }
}

static std::atomic<FILETIME> baselineKernelGetSystemTimeAsFileTime({ 0 });
static std::atomic<FILETIME> baselineDetourGetSystemTimeAsFileTime({ 0 });
static std::atomic<FILETIME> prevcallKernelGetSystemTimeAsFileTime({ 0 });
static std::atomic<FILETIME> prevcallDetourGetSystemTimeAsFileTime({ 0 });
static std::atomic<bool> shouldUpdateGetSystemTimeAsFileTime = false;
VOID WINAPI DetourGetSystemTimeAsFileTime(LPFILETIME lpSystemTimeAsFileTime)
{
    std::shared_lock<std::shared_mutex> lock(mutex);
    if (lpSystemTimeAsFileTime == NULL)
    {
        return;
    }
    if (pre_factor != SpeedFactor())
    {
        pre_factor = SpeedFactor();
        shouldUpdateAll();
    }
    bool expected = true;
    if (shouldUpdateGetSystemTimeAsFileTime.compare_exchange_weak(expected,
                                                                  false))
    {
        baselineKernelGetSystemTimeAsFileTime.store(
            prevcallKernelGetSystemTimeAsFileTime.load());
        baselineDetourGetSystemTimeAsFileTime.store(
            prevcallDetourGetSystemTimeAsFileTime.load());
    }
    // 从全局变量读取基准点快照到线程栈
    FILETIME baselineKernelSnapshot = baselineKernelGetSystemTimeAsFileTime.load();
    ULARGE_INTEGER baselineKernel = { baselineKernelSnapshot.dwLowDateTime,
                                      baselineKernelSnapshot.dwHighDateTime
    };
    FILETIME baselineDetourSnapshot = baselineDetourGetSystemTimeAsFileTime.load();
    ULARGE_INTEGER baselineDetour = { baselineDetourSnapshot.dwLowDateTime,
                                      baselineDetourSnapshot.dwHighDateTime
    };
    FILETIME ftNow = { 0 };
    realGetSystemTimeAsFileTime(&ftNow);
    prevcallKernelGetSystemTimeAsFileTime.store(ftNow);
    ULARGE_INTEGER ulNow = { ftNow.dwLowDateTime, ftNow.dwHighDateTime };
    ULONGLONG delta = SpeedFactor() * (ulNow.QuadPart - baselineKernel.QuadPart);
    ULARGE_INTEGER ulRtn = { 0 };
    ulRtn.QuadPart = baselineDetour.QuadPart + delta;
    prevcallDetourGetSystemTimeAsFileTime.store(
        { ulRtn.LowPart, ulRtn.HighPart });
    (*lpSystemTimeAsFileTime) = { ulRtn.LowPart, ulRtn.HighPart };
}

static std::atomic<FILETIME> baselineKernelGetSystemTimePreciseAsFileTime({ 0 });
static std::atomic<FILETIME> baselineDetourGetSystemTimePreciseAsFileTime({ 0 });
static std::atomic<FILETIME> prevcallKernelGetSystemTimePreciseAsFileTime({ 0 });
static std::atomic<FILETIME> prevcallDetourGetSystemTimePreciseAsFileTime({ 0 });
static std::atomic<bool> shouldUpdateGetSystemTimePreciseAsFileTime = false;
VOID WINAPI
DetourGetSystemTimePreciseAsFileTime(LPFILETIME lpSystemTimeAsFileTime)
{
    std::shared_lock<std::shared_mutex> lock(mutex);
    if (lpSystemTimeAsFileTime == NULL)
    {
        return;
    }
    if (pre_factor != SpeedFactor())
    {
        pre_factor = SpeedFactor();
        shouldUpdateAll();
    }
    bool expected = true;
    if (shouldUpdateGetSystemTimePreciseAsFileTime.compare_exchange_weak(
            expected, false))
    {
        baselineKernelGetSystemTimePreciseAsFileTime.store(
            prevcallKernelGetSystemTimePreciseAsFileTime.load());
        baselineDetourGetSystemTimePreciseAsFileTime.store(
            prevcallDetourGetSystemTimePreciseAsFileTime.load());
    }
    // 从全局变量读取基准点快照到线程栈
    FILETIME baselineKernelSnapshot = baselineKernelGetSystemTimePreciseAsFileTime.load();
    ULARGE_INTEGER baselineKernel = { baselineKernelSnapshot.dwLowDateTime,
                                      baselineKernelSnapshot.dwHighDateTime
    };
    FILETIME baselineDetourSnapshot = baselineDetourGetSystemTimePreciseAsFileTime.load();
    ULARGE_INTEGER baselineDetour = { baselineDetourSnapshot.dwLowDateTime,
                                      baselineDetourSnapshot.dwHighDateTime
    };
    FILETIME ftNow = { 0 };
    realGetSystemTimePreciseAsFileTime(&ftNow);
    prevcallKernelGetSystemTimePreciseAsFileTime.store(ftNow);
    ULARGE_INTEGER ulNow = { ftNow.dwLowDateTime,
                             ftNow.dwHighDateTime
    };
    ULONGLONG delta = SpeedFactor() * (ulNow.QuadPart - baselineKernel.QuadPart);
    ULARGE_INTEGER ulRtn = { 0 };
    ulRtn.QuadPart = baselineDetour.QuadPart + delta;
    prevcallDetourGetSystemTimePreciseAsFileTime.store({ ulRtn.LowPart, ulRtn.HighPart });
    (*lpSystemTimeAsFileTime) = { ulRtn.LowPart, ulRtn.HighPart };
}

BOOL WINAPI DetourSetWaitableTimer(
    HANDLE               hTimer,
    const LARGE_INTEGER* lpDueTime,
    LONG                 lPeriod,
    PTIMERAPCROUTINE     pfnCompletionRoutine,
    LPVOID               lpArgToCompletionRoutine,
    BOOL                 fResume
    )
{
    if (lpDueTime == NULL)
    {
        return FALSE;
    }
    LARGE_INTEGER dueTime = {0};
    dueTime.QuadPart = lpDueTime->QuadPart / SpeedFactor();
    return realSetWaitableTimer(hTimer,
                                &dueTime,
                                lPeriod,
                                pfnCompletionRoutine,
                                lpArgToCompletionRoutine,
                                fResume);
}

BOOL WINAPI DetourSetWaitableTimerEx(
    HANDLE               hTimer,
    const LARGE_INTEGER* lpDueTime,
    LONG                 lPeriod,
    PTIMERAPCROUTINE     pfnCompletionRoutine,
    LPVOID               lpArgToCompletionRoutine,
    PREASON_CONTEXT      WakeContext,
    ULONG                TolerableDelay
    )
{
    if (lpDueTime == NULL)
    {
        return FALSE;
    }
    LARGE_INTEGER dueTime = {0};
    dueTime.QuadPart = lpDueTime->QuadPart / SpeedFactor();
    return realSetWaitableTimerEx(hTimer,
                                       &dueTime,
                                       lPeriod,
                                       pfnCompletionRoutine,
                                       lpArgToCompletionRoutine,
                                       WakeContext,
                                       TolerableDelay);
}

inline VOID shouldUpdateAll()
{
    shouldUpdateTimeGetTime = true;
    shouldUpdateGetMessageTime = true;
    shouldUpdateGetTickCount = true;
    shouldUpdateGetTickCount64 = true;
    shouldUpdateQueryPerformanceCounter = true;
    shouldUpdateGetSystemTimeAsFileTime = true;
    shouldUpdateGetSystemTimePreciseAsFileTime = true;
}

template <typename S, typename T>
inline DWORD SP_CreateHook(DWORD hookId,
                           const wchar_t* hookName,
                           S* pTarget,
                           S* pDetour,
                           T** ppOriginal)
{
    MH_STATUS status = MH_CreateHook(reinterpret_cast<LPVOID>(pTarget),
                                     reinterpret_cast<LPVOID>(pDetour),
                                     reinterpret_cast<LPVOID*>(ppOriginal));
    if (status != MH_OK)
    {
        SP_DbgLog(L"MH_CreateHook name=%s target=%p FAILED status=%s",
                  hookName, (void*)pTarget, SP_MhStatusName(status));
        return SP_EncodeMhError(SP_INIT_HOOK_CREATE_ERROR, status, hookId);
    }
    return ERROR_SUCCESS;
}

SPEEDPATCH_API DWORD WINAPI SP_Initialize(LPVOID)
{
    LONG expected = 0;
    if (!initState.compare_exchange_strong(expected, 1))
    {
        if (expected == 2)
            return ERROR_SUCCESS;
        if (expected == 3)
            return SP_INIT_RESTART_REQUIRED;
        return ERROR_BUSY;
    }

    SP_DbgLog(L"SP_Initialize: begin");
    MH_STATUS status = MH_Initialize();
    SP_DbgLog(L"SP_Initialize: MH_Initialize status=%s", SP_MhStatusName(status));
    if (status != MH_OK)
    {
        initState.store(0);
        return SP_EncodeMhError(SP_INIT_MINHOOK_ERROR, status);
    }

    // Publish INITIALIZING before any potentially slow hook work. The bridge
    // can distinguish this from DISABLED and can cancel enablement on shutdown.
    SP_Install();
    if (pState == nullptr)
    {
        MH_STATUS rollback = MH_Uninitialize();
        if (rollback != MH_OK)
        {
            initState.store(3);
            return SP_EncodeMhError(SP_INIT_ROLLBACK_ERROR, rollback);
        }
        initState.store(0);
        SP_DbgLog(L"SP_Initialize: shared mapping initialization failed");
        return installErrorCode != ERROR_SUCCESS
                   ? installErrorCode
                   : (SP_INIT_MAPPING_CREATE_ERROR | ERROR_GEN_FAILURE);
    }

    FILETIME now = { 0 };
    baselineKernelTimeGetTime = timeGetTime();
    prevcallKernelTimeGetTime = baselineKernelTimeGetTime;
    baselineDetourTimeGetTime = baselineKernelTimeGetTime;
    prevcallDetourTimeGetTime = baselineKernelTimeGetTime;

    baselineKernelGetMessageTime = GetMessageTime();
    prevcallKernelGetMessageTime = baselineKernelGetMessageTime;
    baselineDetourGetMessageTime = baselineKernelGetMessageTime;
    prevcallDetourGetMessageTime = baselineKernelGetMessageTime;

    baselineKernelGetTickCount = GetTickCount();
    prevcallKernelGetTickCount = baselineKernelGetTickCount;
    baselineDetourGetTickCount = baselineKernelGetTickCount;
    prevcallDetourGetTickCount = baselineKernelGetTickCount;

    baselineKernelGetTickCount64 = GetTickCount64();
    prevcallKernelGetTickCount64 = baselineKernelGetTickCount64;
    baselineDetourGetTickCount64 = baselineKernelGetTickCount64;
    prevcallDetourGetTickCount64 = baselineKernelGetTickCount64;

    QueryPerformanceCounter(&baselineKernelQueryPerformanceCounter);
    prevcallKernelQueryPerformanceCounter = baselineKernelQueryPerformanceCounter;
    baselineDetourQueryPerformanceCounter = baselineKernelQueryPerformanceCounter;
    prevcallDetourQueryPerformanceCounter = baselineKernelQueryPerformanceCounter;
    QueryPerformanceFrequency(&baselineKernelQueryPerformanceFrequency);

    GetSystemTimeAsFileTime(&now);
    baselineKernelGetSystemTimeAsFileTime.store(now);
    prevcallKernelGetSystemTimeAsFileTime.store(now);
    baselineDetourGetSystemTimeAsFileTime.store(now);
    prevcallDetourGetSystemTimeAsFileTime.store(now);

    GetSystemTimePreciseAsFileTime(&now);
    baselineKernelGetSystemTimePreciseAsFileTime.store(now);
    prevcallKernelGetSystemTimePreciseAsFileTime.store(now);
    baselineDetourGetSystemTimePreciseAsFileTime.store(now);
    prevcallDetourGetSystemTimePreciseAsFileTime.store(now);

    DWORD hookError = SP_CreateHook(1, L"Sleep", &Sleep, &DetourSleep, &realSleep);
    if (hookError == ERROR_SUCCESS)
        hookError = SP_CreateHook(2, L"SleepEx", &SleepEx, &DetourSleepEx, &realSleepEx);
    if (hookError == ERROR_SUCCESS)
        hookError = SP_CreateHook(3, L"SetWaitableTimer", &SetWaitableTimer,
                                  &DetourSetWaitableTimer, &realSetWaitableTimer);
    if (hookError == ERROR_SUCCESS)
        hookError = SP_CreateHook(4, L"SetWaitableTimerEx", &SetWaitableTimerEx,
                                  &DetourSetWaitableTimerEx, &realSetWaitableTimerEx);
    if (hookError == ERROR_SUCCESS)
        hookError = SP_CreateHook(5, L"SetTimer", &SetTimer, &DetourSetTimer, &realSetTimer);
    if (hookError == ERROR_SUCCESS)
        hookError = SP_CreateHook(6, L"timeGetTime", &timeGetTime,
                                  &DetourTimeGetTime, &realTimeGetTime);
    if (hookError == ERROR_SUCCESS)
        hookError = SP_CreateHook(7, L"timeSetEvent", &timeSetEvent,
                                  &DetourTimeSetEvent, &realTimeSetEvent);
    if (hookError == ERROR_SUCCESS)
        hookError = SP_CreateHook(8, L"GetMessageTime", &GetMessageTime,
                                  &DetourGetMessageTime, &realGetMessageTime);
    if (hookError == ERROR_SUCCESS)
        hookError = SP_CreateHook(9, L"GetTickCount", &GetTickCount,
                                  &DetourGetTickCount, &realGetTickCount);
    if (hookError == ERROR_SUCCESS)
        hookError = SP_CreateHook(10, L"GetTickCount64", &GetTickCount64,
                                  &DetourGetTickCount64, &realGetTickCount64);
    if (hookError == ERROR_SUCCESS)
        hookError = SP_CreateHook(11, L"QueryPerformanceCounter", &QueryPerformanceCounter,
                                  &DetourQueryPerformanceCounter, &realQueryPerformanceCounter);
    if (hookError == ERROR_SUCCESS)
        hookError = SP_CreateHook(12, L"GetSystemTimeAsFileTime", &GetSystemTimeAsFileTime,
                                  &DetourGetSystemTimeAsFileTime,
                                  &realGetSystemTimeAsFileTime);
    if (hookError == ERROR_SUCCESS)
        hookError = SP_CreateHook(13, L"GetSystemTimePreciseAsFileTime",
                                  &GetSystemTimePreciseAsFileTime,
                                  &DetourGetSystemTimePreciseAsFileTime,
                                  &realGetSystemTimePreciseAsFileTime);

    if (hookError != ERROR_SUCCESS)
    {
        SP_Uninstall();
        MH_STATUS rollback = MH_Uninitialize();
        if (rollback != MH_OK)
        {
            initState.store(3);
            return SP_EncodeMhError(SP_INIT_ROLLBACK_ERROR, rollback);
        }
        initState.store(0);
        return hookError;
    }

    status = MH_EnableHook(MH_ALL_HOOKS);
    if (status != MH_OK)
    {
        MH_STATUS disableStatus = MH_DisableHook(MH_ALL_HOOKS);
        // Some hooks may have become active before MinHook reported the
        // failure. Keep the DLL and status mapping resident, and make the
        // failure non-retryable until the target process is restarted.
        SP_WriteState(pState, SP_STATE_FAILED);
        initState.store(3);
        SP_DbgLog(L"SP_Initialize: MH_EnableHook(MH_ALL_HOOKS) FAILED status=%s rollback=%s",
                  SP_MhStatusName(status), SP_MhStatusName(disableStatus));
        if (disableStatus != MH_OK && disableStatus != MH_ERROR_DISABLED)
        {
            return SP_EncodeMhError(SP_INIT_ROLLBACK_ERROR, disableStatus);
        }
        // Keep MinHook's disabled trampolines allocated. A thread may already
        // have entered a detour before rollback; unloading in that state is unsafe.
        return SP_EncodeMhError(SP_INIT_HOOK_ENABLE_ERROR, status);
    }

    SP_CompareAndSetState(pState, SP_STATE_INITIALIZING, SP_STATE_ENABLED);
    initState.store(2);
    SP_DbgLog(L"SP_Initialize: all hooks installed");
    return ERROR_SUCCESS;
}

BOOL APIENTRY DllMain(HMODULE hModule, DWORD reason, LPVOID)
{
    if (reason == DLL_PROCESS_ATTACH)
    {
        DisableThreadLibraryCalls(hModule);
    }
    return TRUE;
}
