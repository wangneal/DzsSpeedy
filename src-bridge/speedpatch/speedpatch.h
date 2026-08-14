/*
 * DzsSpeedy (斗战神游戏加速器) — Windows 时间感知加速控制器
 * Copyright (C) 2026 wangneal
 * https://github.com/wangneal/DzsSpeedy
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */
#ifndef SPEEDPATCH_H
#define SPEEDPATCH_H
#include <windows.h>
#include <string>

// The high bit of the shared hook-thread field means SP_HookProc reached its
// final write. The lower 31 bits contain the callback thread ID.

#if defined(SPEEDPATCH_LIBRARY)
#define SPEEDPATCH_API __declspec(dllexport)
#else
#define SPEEDPATCH_API __declspec(dllimport)
#endif

std::wstring
GetCurrentProcessName();

std::wstring
GetProcessFileMapName(DWORD processId);

extern "C"
{
SPEEDPATCH_API void SP_Install();
SPEEDPATCH_API void SP_Uninstall();
SPEEDPATCH_API DWORD WINAPI SP_Initialize(LPVOID reserved);
SPEEDPATCH_API LRESULT CALLBACK SP_HookProc(int code, WPARAM wParam, LPARAM lParam);
SPEEDPATCH_API BOOL SP_IsEnabled();
SPEEDPATCH_API BOOL SP_IsEnabledById(DWORD processId);
SPEEDPATCH_API void SP_Enable(DWORD processId);
SPEEDPATCH_API void SP_Disable(DWORD processId);
SPEEDPATCH_API void SP_SetSpeed(double factor_);
SPEEDPATCH_API double SP_GetSpeed();
SPEEDPATCH_API DWORD WINAPI SP_Shutdown(LPVOID reserved);
}

#endif // SPEEDPATCH_H
