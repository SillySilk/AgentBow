@echo off
REM ── Stop any running Bow instance and free its port (default 9357) ──
echo Stopping Bow...
taskkill /F /IM bow-desktop.exe >nul 2>&1
REM Defensive: also free anything still LISTENING on 9357.
for /f "tokens=5" %%a in ('netstat -ano ^| findstr ":9357" ^| findstr "LISTENING"') do taskkill /F /PID %%a >nul 2>&1
echo Done. Bow stopped and port 9357 freed.
timeout /t 1 /nobreak >nul
