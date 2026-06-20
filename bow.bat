@echo off
REM Bow Image Studio launcher — builds web UI + backend, then runs.
pushd "%~dp0desktop\webapp"
call npm run build || goto :err
popd
pushd "%~dp0desktop\src-tauri"
cargo build || goto :err
REM Copy built web assets next to the exe so release runs find them.
if not exist "target\debug\web" mkdir "target\debug\web"
xcopy /E /I /Y "..\webapp\dist\*" "target\debug\web\" >nul
start "" "target\debug\bow-desktop.exe"
popd
exit /b 0
:err
echo Build failed.
popd
exit /b 1
