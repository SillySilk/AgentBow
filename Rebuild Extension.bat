@echo off
echo ════════════════════════════════════════
echo   Rebuilding Bow Chrome Extension
echo ════════════════════════════════════════

cd /d "%~dp0extension"

echo Installing dependencies...
call npm install
if errorlevel 1 (
    echo.
    echo NPM INSTALL FAILED — check output above
    pause
    exit /b 1
)

echo Building extension...
call npm run build
if errorlevel 1 (
    echo.
    echo BUILD FAILED — check output above
    pause
    exit /b 1
)

echo.
echo Extension built to: %~dp0extension\dist
echo.
echo Next: Go to chrome://extensions, click "Reload" on Bow AI Agent.
echo.
pause
