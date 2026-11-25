@echo off
rem Follows the newest VIctory run log in a console window.
title VIctory live run log
powershell -NoExit -NoProfile -Command "$runs = Join-Path $env:USERPROFILE 'victory-mcp\runs'; $log = Get-ChildItem -Path $runs -Filter '*.log' | Sort-Object LastWriteTime -Descending | Select-Object -First 1; if (-not $log) { Write-Host ('No run log in ' + $runs); return }; $host.UI.RawUI.WindowTitle = 'VIctory live log: ' + $log.Name; Write-Host ('Following ' + $log.FullName); Get-Content -Path $log.FullName -Tail 40 -Wait"
