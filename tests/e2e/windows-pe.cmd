@echo off
setlocal EnableExtensions EnableDelayedExpansion

if "%~1"=="" (
    echo Usage: windows-pe.cmd ELI_PATH 1>&2
    exit /b 2
)
if /i not "%SystemDrive%"=="X:" (
    echo This test must run in Windows PE. 1>&2
    exit /b 2
)

set "ELI=%~f1"
set "TEST_ROOT=%TEMP%\eli-hook-pe-%RANDOM%-%RANDOM%"
set "HOOK_DIR=%TEST_ROOT%\onExit"
set "MARKER=%TEST_ROOT%\order.txt"
set "STDOUT=%TEST_ROOT%\stdout.txt"
set "STDERR=%TEST_ROOT%\stderr.txt"
set "RESULT=0"

md "%HOOK_DIR%" || exit /b 1
>"%HOOK_DIR%\_Preset.cmd" echo @echo _first^>^>"%MARKER%"
>"%HOOK_DIR%\0-failed.cmd" echo @echo 0-failed^>^>"%MARKER%" ^& exit /b 7
>"%HOOK_DIR%\a-after.cmd" echo @echo a-after^>^>"%MARKER%"

"%ELI%" hook call onExit --dictionary "%TEST_ROOT%" --policy sync >"%STDOUT%" 2>"%STDERR%"
if "%ERRORLEVEL%"=="0" (
    echo A failed hook script did not make the final summary fail. 1>&2
    set "RESULT=1"
    goto cleanup
)

set "LINE1="
set "LINE2="
set "LINE3="
set "LINE4="
<"%MARKER%" (
    set /p "LINE1="
    set /p "LINE2="
    set /p "LINE3="
    set /p "LINE4="
)
if not "!LINE1!"=="_first" set "RESULT=1"
if not "!LINE2!"=="0-failed" set "RESULT=1"
if not "!LINE3!"=="a-after" set "RESULT=1"
if defined LINE4 set "RESULT=1"
if "!RESULT!"=="1" (
    echo Sync hook order or continuation after failure was incorrect. 1>&2
    echo Actual: 1>&2
    type "%MARKER%" 1>&2
    goto cleanup
)
findstr /x /c:"2 succeeded, 1 failed" "%STDOUT%" >nul
if errorlevel 1 (
    echo Hook summary counts were incorrect. 1>&2
    set "RESULT=1"
    goto cleanup
)
findstr /c:"0-failed.cmd" "%STDERR%" >nul
if errorlevel 1 (
    echo Failed hook script was not reported. 1>&2
    set "RESULT=1"
)

:cleanup
rd /s /q "%TEST_ROOT%" 2>nul
exit /b %RESULT%
