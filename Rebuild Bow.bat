@echo off
set EXE=C:\AI\agent Bow\desktop\src-tauri\target\debug\bow-desktop.exe

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

echo Building...
cd /d "%~dp0desktop\src-tauri"
cargo build
if errorlevel 1 (
    echo.
    echo BUILD FAILED — check output above
    pause
    exit /b 1
)

echo Launching...
start "" "%EXE%"
echo Done.
