@echo off
setlocal EnableExtensions

set "ROOT=%~1"
if not defined ROOT exit /b 2
set "ELI=%ROOT%\eli.exe"
set "BOOT=%ROOT%\boot"
set "EDGE=%BOOT%\Edgeless"
set "INPUT=%ROOT%\input"
set "SEVEN=X:\Program Files\7-Zip\7z.exe"
set "PATH=X:\Program Files\7-Zip;%PATH%"

if not exist "%ELI%" exit /b 3
if not exist "%SEVEN%" exit /b 4
if not exist "%EDGE%\version.txt" exit /b 5

rd /s /q "%EDGE%\Default" 2>nul
del /f /q "%EDGE%\wp.jpg" "%EDGE%\wp_backup.jpg" 2>nul
rd /s /q "%INPUT%" 2>nul
md "%INPUT%" || exit /b 6

>"%INPUT%\Classic.esc" echo LOAD %%SystemRoot%%\System32\shell32.dll
"%ELI%" --bootdisk "%BOOT%" theme store "%INPUT%\Classic.esc" || exit /b 10
fc /b "%INPUT%\Classic.esc" "%EDGE%\Default\StartIsBackConfig.esc" >nul || exit /b 11

md "%INPUT%\eis"
>"%INPUT%\eis\icon.png" echo icon
"%SEVEN%" a -t7z "%INPUT%\Standalone.eis" "%INPUT%\eis\*" -y >nul || exit /b 12
"%ELI%" --bootdisk "%BOOT%" theme store "%INPUT%\Standalone.eis" || exit /b 13
fc /b "%INPUT%\Standalone.eis" "%EDGE%\Default\IconPack.eis" >nul || exit /b 14

md "%INPUT%\ems"
>"%INPUT%\ems\cursor.cur" echo cursor
"%SEVEN%" a -t7z "%INPUT%\Standalone.ems" "%INPUT%\ems\*" -y >nul || exit /b 15
"%ELI%" --bootdisk "%BOOT%" theme store "%INPUT%\Standalone.ems" || exit /b 16
fc /b "%INPUT%\Standalone.ems" "%EDGE%\Default\MouseStyle.ems" >nul || exit /b 17

md "%INPUT%\ess"
>"%INPUT%\ess\shell32.dll" echo shell
"%SEVEN%" a -t7z "%INPUT%\Standalone.ess" "%INPUT%\ess\*" -y >nul || exit /b 18
"%ELI%" --bootdisk "%BOOT%" theme store "%INPUT%\Standalone.ess" || exit /b 19
fc /b "%INPUT%\Standalone.ess" "%EDGE%\Default\SystemIconPack.ess" >nul || exit /b 20

md "%EDGE%\Default\LoadScreen\nested"
>"%EDGE%\Default\LoadScreen\old.jpg" echo old
>"%EDGE%\Default\LoadScreen\keep.ini" echo keep
>"%EDGE%\Default\LoadScreen\nested\old.jpg" echo nested
md "%INPUT%\els"
>"%INPUT%\els\new.jpg" echo new
"%SEVEN%" a -t7z "%INPUT%\Legacy.els" "%INPUT%\els\*" -y >nul || exit /b 21
"%ELI%" --bootdisk "%BOOT%" theme store "%INPUT%\Legacy.els" || exit /b 22
if exist "%EDGE%\Default\LoadScreen\old.jpg" exit /b 23
if not exist "%EDGE%\Default\LoadScreen\new.jpg" exit /b 24
if not exist "%EDGE%\Default\LoadScreen\keep.ini" exit /b 25
if not exist "%EDGE%\Default\LoadScreen\nested\old.jpg" exit /b 26

md "%INPUT%\eth"
copy /y "%INPUT%\Standalone.eis" "%INPUT%\eth\IconPack.eis" >nul
copy /y "%INPUT%\Standalone.ems" "%INPUT%\eth\MouseStyle.ems" >nul
copy /y "%INPUT%\Standalone.ess" "%INPUT%\eth\SystemIconPack.ess" >nul
copy /y "%INPUT%\Legacy.els" "%INPUT%\eth\LoadScreen.els" >nul
copy /y "%INPUT%\Classic.esc" "%INPUT%\eth\StartIsBackConfig.esc" >nul
>"%INPUT%\eth\WallPaper.jpg" echo theme-wallpaper
>"%EDGE%\Default\stale.txt" echo stale
"%SEVEN%" a -t7z "%INPUT%\Complete.eth" "%INPUT%\eth\*" -y >nul || exit /b 27
"%ELI%" --bootdisk "%BOOT%" theme store "%INPUT%\Complete.eth" || exit /b 28
if exist "%EDGE%\Default\stale.txt" exit /b 29
if not exist "%EDGE%\Default\IconPack.eis" exit /b 30
if not exist "%EDGE%\Default\MouseStyle.ems" exit /b 31
if not exist "%EDGE%\Default\SystemIconPack.ess" exit /b 32
if not exist "%EDGE%\Default\StartIsBackConfig.esc" exit /b 33
if not exist "%EDGE%\Default\LoadScreen\new.jpg" exit /b 34
fc /b "%INPUT%\eth\WallPaper.jpg" "%EDGE%\wp.jpg" >nul || exit /b 35

md "%INPUT%\partial"
copy /y "%INPUT%\Classic.esc" "%INPUT%\partial\StartIsBackConfig.esc" >nul
"%SEVEN%" a -t7z "%INPUT%\Partial.eth" "%INPUT%\partial\*" -y >nul || exit /b 36
"%ELI%" --bootdisk "%BOOT%" theme store "%INPUT%\Partial.eth" || exit /b 37
if exist "%EDGE%\Default\IconPack.eis" exit /b 38
if not exist "%EDGE%\Default\StartIsBackConfig.esc" exit /b 39
if not exist "%EDGE%\Default\Info.txt" exit /b 40
for /f %%C in ('find /v /c "" ^< "%EDGE%\Default\Info.txt"') do set "INFO_LINES=%%C"
if not "%INFO_LINES%"=="5" exit /b 41
if exist "%EDGE%\wp.jpg" exit /b 42
fc /b "%INPUT%\eth\WallPaper.jpg" "%EDGE%\wp_backup.jpg" >nul || exit /b 43

>"%INPUT%\Seed.jpg" echo seed-wallpaper
>"%INPUT%\First.jpg" echo first-wallpaper
"%ELI%" --bootdisk "%BOOT%" theme store "%INPUT%\Seed.jpg" || exit /b 44
"%ELI%" --bootdisk "%BOOT%" theme store "%INPUT%\First.jpg" || exit /b 45
fc /b "%INPUT%\First.jpg" "%EDGE%\wp.jpg" >nul || exit /b 46
fc /b "%INPUT%\Seed.jpg" "%EDGE%\wp_backup.jpg" >nul || exit /b 47

"%ELI%" --bootdisk "%BOOT%" theme list >"%ROOT%\list.txt" || exit /b 48
findstr /r /c:"Resource       *Configured" "%ROOT%\list.txt" >nul || exit /b 66
findstr /r /c:"Icon Pack      *No" "%ROOT%\list.txt" >nul || exit /b 67
findstr /r /c:"System Icons   *No" "%ROOT%\list.txt" >nul || exit /b 68
findstr /r /c:"Start Menu     *Yes" "%ROOT%\list.txt" >nul || exit /b 49
findstr /r /c:"Wallpaper      *Yes" "%ROOT%\list.txt" >nul || exit /b 50
findstr /r /c:"LoadScreen     *No" "%ROOT%\list.txt" >nul || exit /b 51
findstr /r /c:"Mouse Style    *No" "%ROOT%\list.txt" >nul || exit /b 69
findstr /i /r /c:"complete" /c:"partial" /c:"legacy" "%ROOT%\list.txt" >nul && exit /b 70
"%ELI%" --bootdisk "%BOOT%" theme delete esc || exit /b 52
if exist "%EDGE%\Default\StartIsBackConfig.esc" exit /b 53
"%ELI%" --bootdisk "%BOOT%" theme delete jpg || exit /b 54
if exist "%EDGE%\wp.jpg" exit /b 55
if not exist "%EDGE%\wp_backup.jpg" exit /b 56

"%ELI%" --bootdisk "%BOOT%" theme store "%INPUT%\Complete.eth" || exit /b 57
"%ELI%" --bootdisk "%BOOT%" theme store "%INPUT%\Seed.jpg" || exit /b 71
"%ELI%" --bootdisk "%BOOT%" theme delete all || exit /b 58
if exist "%EDGE%\Default\IconPack.eis" exit /b 59
if exist "%EDGE%\Default\SystemIconPack.ess" exit /b 60
if exist "%EDGE%\Default\LoadScreen" exit /b 61
if exist "%EDGE%\Default\MouseStyle.ems" exit /b 62
if exist "%EDGE%\Default\StartIsBackConfig.esc" exit /b 63
if exist "%EDGE%\wp.jpg" exit /b 64
if not exist "%EDGE%\wp_backup.jpg" exit /b 65

dir /b "%EDGE%\.eli-theme-store-*" >nul 2>nul && exit /b 66
echo THEME_STORE_PE_E2E_PASS
exit /b 0
