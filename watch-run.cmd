@echo off
rem Follows the newest VIctory run log in a console window.
rem Run logs are in D:\victory-data\runs (the harness's prologue.DATA_ROOT) or, from before, in %USERPROFILE%\victory-mcp\runs.
title VIctory live run log
powershell -NoExit -NoProfile -Command "$dirs = @('D:\victory-data\runs', (Join-Path $env:USERPROFILE 'victory-mcp\runs')) | Where-Object { Test-Path $_ }; $log = $dirs | ForEach-Object { Get-ChildItem -Path $_ -Filter '*.log' } | Sort-Object LastWriteTime -Descending | Select-Object -First 1; if (-not $log) { Write-Host ('No run log in ' + ($dirs -join ', ')); return }; $host.UI.RawUI.WindowTitle = 'VIctory live log: ' + $log.Name; Write-Host ('Following ' + $log.FullName); Get-Content -Path $log.FullName -Tail 40 -Wait"
