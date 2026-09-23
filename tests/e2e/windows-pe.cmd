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
set "TEST_NAME=eli-hook-pe-%RANDOM%-%RANDOM%"
set "TEST_ROOT=%TEMP%\%TEST_NAME%"
md "%TEST_ROOT%" 2>nul
if not exist "%TEST_ROOT%\" (
    rem 某些通过受限调试服务启动的 PE 进程不能写系统 TEMP，回退到 ELI 同卷目录。
    set "TEST_ROOT=%~dp1%TEST_NAME%"
    md "!TEST_ROOT!" 2>nul
)
if not exist "%TEST_ROOT%\" (
    echo Unable to create the Windows PE E2E test directory. 1>&2
    exit /b 1
)
set "HOOK_DIR=%TEST_ROOT%\onExit"
set "MARKER=%TEST_ROOT%\order.txt"
set "STDOUT=%TEST_ROOT%\stdout.txt"
set "STDERR=%TEST_ROOT%\stderr.txt"
set "RESULT=0"

md "%HOOK_DIR%" || exit /b 1
>"%HOOK_DIR%\_Preset.cmd" echo @echo _first^>^>"%MARKER%"
>"%HOOK_DIR%\0-failed.cmd" echo @echo 0-failed^>^>"%MARKER%"
>>"%HOOK_DIR%\0-failed.cmd" echo @exit /b 7
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
findstr /c:"2 succeeded, 1 failed" "%STDOUT%" >nul
if errorlevel 1 (
    echo Hook summary counts were incorrect. 1>&2
    echo Standard output: 1>&2
    type "%STDOUT%" 1>&2
    echo Standard error: 1>&2
    type "%STDERR%" 1>&2
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
for %%i in (7z.exe) do set "SEVENZIP=%%~$PATH:i"
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
findstr /c:"0 applied, 0 applied with warnings, 1 skipped, 0 failed" "%STDOUT%" >nul
if errorlevel 1 (
    echo ELS-only eth summary is incorrect. 1>&2
    set "RESULT=1"
)
findstr /c:"legacy LoadScreen.els" "%STDERR%" >nul
if errorlevel 1 (
    echo ELS-only eth migration warning is missing. 1>&2
    set "RESULT=1"
)

rem 最小 ESC：先写入标记，再强制重启 Explorer，标记必须保留。
> "%THEME_DIR%\minimal.esc" echo REGI #HKCU\Software\Edgeless\EliThemeE2E\\EscApplied=1
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
reg query HKCU\Software\Edgeless\EliThemeE2E /v EscApplied 2>nul | findstr /i /c:"0x1" >nul
if errorlevel 1 (
    echo ESC registry marker did not survive the Explorer restart. 1>&2
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

rem 独立 JPG：使用镜像自带 JPEG，验证稳定路径发布和 PECMD WALL。
set "WALLPAPER_SOURCE=%SystemRoot%\Web\Wallpaper\Windows\img0.jpg"
if not exist "%WALLPAPER_SOURCE%" set "WALLPAPER_SOURCE=%SystemRoot%\System32\winpe.jpg"
if not exist "%WALLPAPER_SOURCE%" (
    echo No built-in JPEG is available for the wallpaper test. 1>&2
    set "RESULT=1"
    goto theme_done
)
copy /y "%WALLPAPER_SOURCE%" "%THEME_DIR%\WallPaper.jpg" >nul
"%ELI%" theme apply "%THEME_DIR%\WallPaper.jpg" >"%STDOUT%" 2>"%STDERR%"
if not "!ERRORLEVEL!"=="0" (
    echo Wallpaper theme apply should succeed. 1>&2
    set "RESULT=1"
    goto theme_done
)
findstr /c:"Applied WallPaper.jpg" "%STDOUT%" >nul
if errorlevel 1 (
    echo Wallpaper success output was not reported. 1>&2
    type "%STDOUT%" 1>&2
    set "RESULT=1"
)

rem 最小 EIS：动态选取桌面第一层现有 LNK，并用镜像自带 ICO 更新它。
set "DESKTOP_LNK="
for %%i in ("%USERPROFILE%\Desktop\*.lnk") do if not defined DESKTOP_LNK if exist "%%~fi" set "DESKTOP_LNK=%%~ni"
if not defined DESKTOP_LNK for %%i in ("%SystemDrive%\Users\Default\Desktop\*.lnk") do if not defined DESKTOP_LNK if exist "%%~fi" set "DESKTOP_LNK=%%~ni"
set "ICON_SOURCE=%SystemDrive%\Users\Icon\type\eth.ico"
if not exist "%ICON_SOURCE%" set "ICON_SOURCE=%SystemRoot%\System32\transparent.ico"
if not defined DESKTOP_LNK (
    echo No desktop LNK is available for the EIS test. 1>&2
    set "RESULT=1"
    goto theme_done
)
if not exist "%ICON_SOURCE%" (
    echo No built-in ICO is available for the EIS test. 1>&2
    set "RESULT=1"
    goto theme_done
)
md "%THEME_DIR%\eis\shortcut" 2>nul
copy /y "%ICON_SOURCE%" "%THEME_DIR%\eis\shortcut\!DESKTOP_LNK!.ico" >nul
pushd "%THEME_DIR%\eis"
"%SEVENZIP%" a "%TEST_ROOT%\minimal.eis" "shortcut\*.ico" >nul 2>nul
if errorlevel 1 (
    echo Failed to build the EIS package. 1>&2
    set "RESULT=1"
    popd
    goto theme_done
)
popd
"%ELI%" theme apply "%TEST_ROOT%\minimal.eis" >"%STDOUT%" 2>"%STDERR%"
if not "!ERRORLEVEL!"=="0" (
    echo EIS theme apply should succeed. 1>&2
    set "RESULT=1"
    goto theme_done
)
findstr /c:"Applied IconPack.eis" "%STDOUT%" >nul
if errorlevel 1 (
    echo EIS success output was not reported. 1>&2
    type "%STDOUT%" 1>&2
    set "RESULT=1"
)
findstr /c:"1 updated, 0 unmatched, 0 failed" "%STDOUT%" >nul
if errorlevel 1 (
    echo EIS shortcut counts were incorrect. 1>&2
    type "%STDOUT%" 1>&2
    set "RESULT=1"
)

rem 最小 EMS：用一个可加载的系统 CUR 复制出 15 个基础和 2 个可选槽位。
set "CURSOR_SOURCE=%SystemRoot%\Cursors\aero_arrow.cur"
if not exist "%CURSOR_SOURCE%" (
    echo The built-in aero_arrow.cur is unavailable for the EMS test. 1>&2
    set "RESULT=1"
    goto theme_done
)
md "%THEME_DIR%\ems" 2>nul
for %%n in (aero_arrow aero_helpsel aero_working aero_busy aero_cross aero_beam aero_pen aero_unavail aero_ns aero_ew aero_nwse aero_nesw aero_move aero_up aero_link aero_pin aero_person) do copy /y "%CURSOR_SOURCE%" "%THEME_DIR%\ems\%%n.cur" >nul
pushd "%THEME_DIR%\ems"
"%SEVENZIP%" a "%TEST_ROOT%\minimal.ems" "*.cur" >nul 2>nul
if errorlevel 1 (
    echo Failed to build the EMS package. 1>&2
    set "RESULT=1"
    popd
    goto theme_done
)
popd
"%ELI%" theme apply "%TEST_ROOT%\minimal.ems" >"%STDOUT%" 2>"%STDERR%"
if not "!ERRORLEVEL!"=="0" (
    echo EMS theme apply should succeed. 1>&2
    set "RESULT=1"
    goto theme_done
)
findstr /c:"Applied MouseStyle.ems" "%STDOUT%" >nul
if errorlevel 1 (
    echo EMS success output was not reported. 1>&2
    type "%STDOUT%" 1>&2
    set "RESULT=1"
)
findstr /c:"cursors refreshed" "%STDOUT%" >nul
if errorlevel 1 (
    echo EMS cursor refresh was not reported. 1>&2
    type "%STDOUT%" 1>&2
    set "RESULT=1"
)

rem 最小 ESS：使用当前系统 DLL 自身验证 Shell-off 双文件替换，不改变最终内容。
md "%THEME_DIR%\ess" 2>nul
copy /y "%SystemRoot%\System32\imageres.dll" "%THEME_DIR%\ess\imageres.dll" >nul
copy /y "%SystemRoot%\System32\imagesp1.dll" "%THEME_DIR%\ess\imagesp1.dll" >nul
pushd "%THEME_DIR%\ess"
"%SEVENZIP%" a "%TEST_ROOT%\minimal.ess" imageres.dll imagesp1.dll >nul 2>nul
if errorlevel 1 (
    echo Failed to build the ESS package. 1>&2
    set "RESULT=1"
    popd
    goto theme_done
)
popd
"%ELI%" theme apply "%TEST_ROOT%\minimal.ess" >"%STDOUT%" 2>"%STDERR%"
if not "!ERRORLEVEL!"=="0" (
    echo ESS theme apply should succeed. 1>&2
    set "RESULT=1"
    goto theme_done
)
findstr /c:"Applied SystemIconPack.ess" "%STDOUT%" >nul
if errorlevel 1 (
    echo ESS success output was not reported. 1>&2
    type "%STDOUT%" 1>&2
    set "RESULT=1"
)
findstr /c:"explorer restarted, icon cache invalidated" "%STDOUT%" >nul
if errorlevel 1 (
    echo ESS Shell refresh output was incorrect. 1>&2
    type "%STDOUT%" 1>&2
    set "RESULT=1"
)

rem 六组件 ETH：验证完整预检、ELS 跳过和统一刷新合并。
md "%THEME_DIR%\eth" 2>nul
copy /y "%THEME_DIR%\WallPaper.jpg" "%THEME_DIR%\eth\WallPaper.jpg" >nul
copy /y "%THEME_DIR%\LoadScreen.els" "%THEME_DIR%\eth\LoadScreen.els" >nul
copy /y "%TEST_ROOT%\minimal.eis" "%THEME_DIR%\eth\IconPack.eis" >nul
copy /y "%TEST_ROOT%\minimal.ems" "%THEME_DIR%\eth\MouseStyle.ems" >nul
copy /y "%THEME_DIR%\minimal.esc" "%THEME_DIR%\eth\StartIsBackConfig.esc" >nul
copy /y "%TEST_ROOT%\minimal.ess" "%THEME_DIR%\eth\SystemIconPack.ess" >nul
pushd "%THEME_DIR%\eth"
"%SEVENZIP%" a "%TEST_ROOT%\complete.eth" WallPaper.jpg LoadScreen.els IconPack.eis MouseStyle.ems StartIsBackConfig.esc SystemIconPack.ess >nul 2>nul
if errorlevel 1 (
    echo Failed to build the complete ETH package. 1>&2
    set "RESULT=1"
    popd
    goto theme_done
)
popd
"%ELI%" theme apply "%TEST_ROOT%\complete.eth" >"%STDOUT%" 2>"%STDERR%"
if not "!ERRORLEVEL!"=="0" (
    echo Complete ETH theme apply should succeed. 1>&2
    set "RESULT=1"
    goto theme_done
)
findstr /c:"5 applied, 0 applied with warnings, 1 skipped, 0 failed" "%STDOUT%" >nul
if errorlevel 1 (
    echo Complete ETH summary was incorrect. 1>&2
    type "%STDOUT%" 1>&2
    set "RESULT=1"
)
findstr /c:"explorer restarted, icon cache invalidated, cursors refreshed" "%STDOUT%" >nul
if errorlevel 1 (
    echo Complete ETH merged refresh output was incorrect. 1>&2
    type "%STDOUT%" 1>&2
    set "RESULT=1"
)
findstr /c:"shortcut(s) notified" "%STDOUT%" >nul
if not errorlevel 1 (
    echo Complete ETH should suppress per-shortcut notifications. 1>&2
    type "%STDOUT%" 1>&2
    set "RESULT=1"
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
reg delete HKCU\Software\Edgeless\EliThemeE2E /f >nul 2>nul

:cleanup
rd /s /q "%TEST_ROOT%" 2>nul
exit /b %RESULT%
