@echo off
setlocal
title Qidomin Build
cd /d "%~dp0"

echo ============================================
echo   Qidomin one-click build
echo ============================================
echo.

where npm >nul 2>nul
if errorlevel 1 (
    echo [ERROR] npm not found. Please install Node.js 22+ first.
    pause
    exit /b 1
)

echo Select build target:
echo   [1] 64-bit  (Win10/11 x64, default)
echo   [2] 32-bit  (x86, for 32-bit Windows 8/10)
echo   [3] Both
choice /c 123 /n /m "Choose (1/2/3): "
if errorlevel 3 goto BOTH
if errorlevel 2 goto X86
goto X64

:X64
call :build
if errorlevel 1 goto FAIL
goto DONE

:X86
call :build -- --target i686-pc-windows-msvc
if errorlevel 1 goto FAIL
goto DONE

:BOTH
call :build
if errorlevel 1 goto FAIL
call :build -- --target i686-pc-windows-msvc
if errorlevel 1 goto FAIL
goto DONE

:build
echo.
echo [1/2] Stopping running Qidomin process...
rem 先礼貌地请它退出：qidomin 收到关闭通知会先把桌面的文件还回去再退出。
rem 只有它自己关不掉（卡死）时才动用 /f 强杀。
tasklist /fi "imagename eq qidomin.exe" 2>nul | find /i "qidomin.exe" >nul
if errorlevel 1 goto STOPPED
taskkill /im qidomin.exe >nul 2>nul
rem 最多等 8 秒，让它把文件搬完
timeout /t 8 /nobreak >nul
tasklist /fi "imagename eq qidomin.exe" 2>nul | find /i "qidomin.exe" >nul
if not errorlevel 1 taskkill /f /im qidomin.exe >nul 2>nul
:STOPPED
timeout /t 1 /nobreak >nul

echo [2/2] Building frontend + Rust release + NSIS installer (2-5 min)...
echo.
call npm run tauri build %~1 %~2
if errorlevel 1 (
    echo.
    echo [FAILED] Build error. Please send the log above to the assistant.
    echo.
    pause
    exit /b 1
)
exit /b 0

:DONE
echo.
echo ============================================
echo   Output files:
echo     64-bit Portable exe:  %~dp0src-tauri\target\release\qidomin.exe
echo     64-bit Installer:     %~dp0src-tauri\target\release\bundle\nsis\*-x64-setup.exe
echo     32-bit Portable exe:  %~dp0src-tauri\target\i686-pc-windows-msvc\release\qidomin.exe
echo     32-bit Installer:     %~dp0src-tauri\target\i686-pc-windows-msvc\release\bundle\nsis\*-x86-setup.exe
echo ============================================
echo.
pause
exit /b 0

:FAIL
pause
exit /b 1
