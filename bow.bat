@echo off
REM Bow Image Studio launcher — builds web UI + backend, then runs.
REM Stop any running instance first so the binary can be relinked (no "Access denied").
REM /T kills the whole tree so the child llama-server can't be orphaned and lock its exe.
taskkill /F /T /IM bow-desktop.exe >nul 2>&1
REM Belt-and-suspenders: reap any orphaned llama-server from a prior crashed/aborted run.
taskkill /F /IM llama-server.exe >nul 2>&1
REM ~1s pause that works under a non-interactive shell (timeout needs a real console).
ping -n 2 127.0.0.1 >nul
pushd "%~dp0desktop\webapp"
call npm run build || goto :err
popd
pushd "%~dp0desktop\src-tauri"
cargo build || goto :err
REM Copy built web assets next to the exe so release runs find them.
if not exist "target\debug\web" mkdir "target\debug\web"
xcopy /E /I /Y "..\webapp\dist\*" "target\debug\web\" >nul || goto :err
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0get-llama.ps1" || goto :err
if not exist "target\debug\llama" mkdir "target\debug\llama"
xcopy /E /I /Y "bin\llama\*" "target\debug\llama\" >nul || goto :err
start "" "target\debug\bow-desktop.exe"
popd
exit /b 0
:err
echo Build failed.
pause
popd
exit /b 1
