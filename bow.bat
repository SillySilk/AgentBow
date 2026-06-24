@echo off
REM Bow Image Studio launcher — builds web UI + backend, then runs.
REM Stop any running instance first so the binary can be relinked (no "Access denied").
taskkill /F /IM bow-desktop.exe >nul 2>&1
timeout /t 1 /nobreak >nul
pushd "%~dp0desktop\webapp"
call npm run build || goto :err
popd
pushd "%~dp0desktop\src-tauri"
cargo build || goto :err
REM Copy built web assets next to the exe so release runs find them.
if not exist "target\debug\web" mkdir "target\debug\web"
xcopy /E /I /Y "..\webapp\dist\*" "target\debug\web\" >nul || goto :err
start "" "target\debug\bow-desktop.exe"
popd
exit /b 0
:err
echo Build failed.
pause
popd
exit /b 1
