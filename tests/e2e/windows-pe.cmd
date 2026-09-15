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

rem ---------------------------------------------------------------------------
rem eli theme apply：仅 Windows PE 实机（构造最小包验证跳过/告警/ESC 应用）
rem ---------------------------------------------------------------------------

set "THEME_DIR=%TEST_ROOT%\theme"
md "%THEME_DIR%" || exit /b 1

rem 独立 ELS：识别、告警、跳过，不解析 7-Zip。
> "%THEME_DIR%\LoadScreen.els" echo legacy-loadscreen
"%ELI%" theme apply "%THEME_DIR%\LoadScreen.els" >"%STDOUT%" 2>"%STDERR%"
if not "%ERRORLEVEL%"=="0" (
    echo Standalone ELS theme apply should succeed. 1>&2
    set "RESULT=1"
    goto theme_done
)
findstr /c:"Skipped LoadScreen.els" "%STDOUT%" >nul
if errorlevel 1 (
    echo Standalone ELS was not reported as skipped. 1>&2
    set "RESULT=1"
)
findstr /c:"legacy LoadScreen.els" "%STDERR%" >nul
if errorlevel 1 (
    echo Standalone ELS migration warning is missing. 1>&2
    set "RESULT=1"
)

rem 最小 .eth（仅 LoadScreen.els）：0 Applied / 1 Skipped。
set "SEVENZIP="
for /f "delims=" %%i in ('where 7z.exe 2^>nul') do if not defined SEVENZIP set "SEVENZIP=%%i"
if not defined SEVENZIP (
    echo 7z.exe is unavailable in the PE test environment. 1>&2
    set "RESULT=1"
    goto theme_done
)
pushd "%THEME_DIR%"
"%SEVENZIP%" a "%TEST_ROOT%\loadscreen-only.eth" LoadScreen.els >nul 2>nul
if errorlevel 1 (
    echo Failed to build the ELS-only eth package. 1>&2
    set "RESULT=1"
    popd
    goto theme_done
)
popd
"%ELI%" theme apply "%TEST_ROOT%\loadscreen-only.eth" >"%STDOUT%" 2>"%STDERR%"
if not "%ERRORLEVEL%"=="0" (
    echo ELS-only eth theme apply should succeed. 1>&2
    set "RESULT=1"
    goto theme_done
)
findstr /x /c:"0 applied, 0 applied with warnings, 1 skipped, 0 failed" "%STDOUT%" >nul
if errorlevel 1 (
    echo ELS-only eth summary is incorrect. 1>&2
    set "RESULT=1"
)
findstr /c:"legacy LoadScreen.els" "%STDERR%" >nul
if errorlevel 1 (
    echo ELS-only eth migration warning is missing. 1>&2
    set "RESULT=1"
)

rem 最小 ESC：PECMD LOAD 应成功，并触发一次 Explorer 重启。
> "%THEME_DIR%\minimal.esc" echo EXIT
"%ELI%" theme apply "%THEME_DIR%\minimal.esc" >"%STDOUT%" 2>"%STDERR%"
if not "%ERRORLEVEL%"=="0" (
    echo Minimal ESC theme apply should succeed. 1>&2
    set "RESULT=1"
    goto theme_done
)
findstr /c:"Applied StartIsBackConfig.esc" "%STDOUT%" >nul
if errorlevel 1 (
    echo Minimal ESC was not applied. 1>&2
    set "RESULT=1"
)
findstr /c:"explorer restarted" "%STDOUT%" >nul
if errorlevel 1 (
    echo Minimal ESC did not restart Explorer exactly once. 1>&2
    set "RESULT=1"
)
set "THEME_LOG=%SystemDrive%\Users\Theme\eli\theme-apply.log"
if not exist "%THEME_LOG%" (
    echo Theme apply event log was not created. 1>&2
    set "RESULT=1"
) else (
    set "LOG_RESULT=0"
    findstr /c:"component=StartIsBackConfig.esc" "%THEME_LOG%" >nul || set "LOG_RESULT=1"
    findstr /c:"phase=commit" "%THEME_LOG%" >nul || set "LOG_RESULT=1"
    findstr /c:"result=applied" "%THEME_LOG%" >nul || set "LOG_RESULT=1"
    findstr /c:"windows_error_code=" "%THEME_LOG%" >nul || set "LOG_RESULT=1"
    if "!LOG_RESULT!"=="1" (
        echo Theme apply event log is missing required event fields. 1>&2
        set "RESULT=1"
    )
)

rem 未知扩展名在任何副作用前拒绝。
> "%TEST_ROOT%\unknown.txt" echo nonsense
"%ELI%" theme apply "%TEST_ROOT%\unknown.txt" >"%STDOUT%" 2>"%STDERR%"
if "%ERRORLEVEL%"=="0" (
    echo Unknown theme extension should fail. 1>&2
    set "RESULT=1"
)
findstr /c:"unsupported theme package extension" "%STDERR%" >nul
if errorlevel 1 (
    echo Unknown theme extension was not rejected. 1>&2
    set "RESULT=1"
)

:theme_done

:cleanup
rd /s /q "%TEST_ROOT%" 2>nul
exit /b %RESULT%
