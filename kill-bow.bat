@echo off
REM ── Stop any running Bow instance and free its port (default 9357) ──
echo Stopping Bow...
REM /T kills the whole tree; the extra kill reaps any llama-server orphaned by a prior run.
taskkill /F /T /IM bow-desktop.exe >nul 2>&1
taskkill /F /IM llama-server.exe >nul 2>&1
REM Defensive: also free anything still LISTENING on 9357.
for /f "tokens=5" %%a in ('netstat -ano ^| findstr ":9357" ^| findstr "LISTENING"') do taskkill /F /PID %%a >nul 2>&1
REM The scraper browser is an Edge child on Bow's own profile; /T misses it once Bow itself is gone,
REM and an orphan keeps the profile locked so the next Bow can't launch a browser.
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"name='msedge.exe'\" | Where-Object { $_.CommandLine -like '*.bow_browser_profile*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }" >nul 2>&1
echo Done. Bow stopped and port 9357 freed.
REM ~1s pause that works under a non-interactive shell (timeout needs a real console).
ping -n 2 127.0.0.1 >nul
