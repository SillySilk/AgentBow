@echo off
echo ════════════════════════════════════════
echo   Rebuilding Bow — Extension + Desktop
echo ════════════════════════════════════════

:: ── Extension ────────────────────────────
echo.
echo [1/2] Building Chrome Extension...
cd /d "%~dp0extension"
call npm install
if errorlevel 1 (
    echo EXTENSION NPM INSTALL FAILED
    pause
    exit /b 1
)
call npm run build
if errorlevel 1 (
    echo EXTENSION BUILD FAILED
    pause
    exit /b 1
)
echo Extension OK.

:: ── Desktop ──────────────────────────────
echo.
echo [2/2] Building Desktop (Rust)...

set EXE=%~dp0desktop\src-tauri\target\debug\bow-desktop.exe

echo Stopping Bow...
taskkill /F /IM bow-desktop.exe /T 2>nul

echo Checking port 9357...
:portloop
for /f "tokens=5" %%p in ('netstat -ano ^| findstr "127.0.0.1:9357.*LISTENING" 2^>nul') do (
    echo Port 9357 held by PID %%p — killing...
    taskkill /F /PID %%p 2>nul
)
timeout /t 2 /nobreak >nul
netstat -ano | findstr "127.0.0.1:9357.*LISTENING" >nul 2>nul && (
    echo Port still held, retrying...
    goto portloop
)
echo Port 9357 is free.

echo Removing old exe...
del /f "%EXE%" 2>nul

cd /d "%~dp0desktop\src-tauri"
cargo build
if errorlevel 1 (
    echo.
    echo DESKTOP BUILD FAILED
    pause
    exit /b 1
)

echo.
echo Launching...
start "" "%EXE%"

echo.
echo ════════════════════════════════════════
echo   All done. Reload extension in Chrome.
echo ════════════════════════════════════════
pause
