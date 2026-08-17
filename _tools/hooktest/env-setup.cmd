@echo off
set LOG=D:\DzsSpeedy\_tools\hooktest\choco-zig.log
echo === start %DATE% %TIME% === >> "%LOG%"
"C:\ProgramData\chocolatey\bin\choco.exe" install zig -y --no-progress >> "%LOG%" 2>&1
echo ZIG_EXIT=%ERRORLEVEL% >> "%LOG%"
"C:\ProgramData\chocolatey\bin\choco.exe" install mingw -y --no-progress >> "%LOG%" 2>&1
echo MINGW_EXIT=%ERRORLEVEL% >> "%LOG%"
echo === end %DATE% %TIME% === >> "%LOG%"
