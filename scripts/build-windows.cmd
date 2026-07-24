@echo off
REM One-click wrapper: double-click this, or run from cmd, to build the
REM Windows installer. Forwards any args to build-windows.ps1
REM (e.g. build-windows.cmd -DebugProfile   or   build-windows.cmd -Version 0.2.0).
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-windows.ps1" %*
set EXITCODE=%ERRORLEVEL%
echo.
pause
exit /b %EXITCODE%
