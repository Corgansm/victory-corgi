# Starts the title on the VIctory-NVIDIA-HighRAM build (Loydben/VIctory-NVIDIA-HighRAM) with the settings of
# its own scripts\play-highram.ps1, a run log in D:\victory-data\runs and a console window that follows the log.
#   run-highram.ps1                 their defaults: Series S, VICTORY_CARD_ALL and VICTORY_FETCH_CHECK on
#   run-highram.ps1 -HighRam:$false without those two (their advice for smaller cards)
#   run-highram.ps1 -SeriesX        the Series X profile
#   run-highram.ps1 -HalfScale      the world at half the Series S size
#   run-highram.ps1 -More @{VICTORY_X='1'}
# Nothing of the game is written below the source tree: the cache is D:\victory-data\cache-highram.
param(
    [bool]$HighRam = $true,
    [switch]$SeriesX,
    [switch]$HalfScale,
    [hashtable]$More = @{},
    [string]$Repo = 'C:\Users\Corgan\Desktop\VIctory-NVIDIA-HighRAM-main',
    [string]$Exe = 'D:\GTA 6 Xbox Build\GTA6.Xbox.v1.0.2.1-CYBERLEEK\game_scarlett_master.exe',
    [string]$Data = 'D:\victory-data'
)
$ErrorActionPreference = 'Stop'
$victory = Join-Path $Repo 'target\diag\victory.exe'
if (-not (Test-Path $victory)) { Write-Output "no ${victory}: build it first"; exit 2 }
if (-not (Test-Path $Exe)) { Write-Output "no $Exe"; exit 2 }
if (Get-Process victory -ErrorAction SilentlyContinue) { Write-Output 'a victory.exe is running already'; exit 3 }

# Their play set (scripts\play-highram.ps1), word for word.
$settings = @{
    VICTORY_TEXTURE_NO_WAIT = '1'; VICTORY_RELAY = '2'; VICTORY_WIDE_BACK = '1'; VICTORY_CARD_REHASH_RUNS = '1'
    VICTORY_JOIN_COPIES = '1'; VICTORY_INDIRECT_WINDOW = '1'; VICTORY_RELAY_COPY = '1'; VICTORY_CARD_STEPS = '2'
    VICTORY_INDIRECT_BATCH = '2'; VICTORY_MAP_THREADS = '1'; VICTORY_TITLE_PRIORITIES = '0'; VICTORY_CARD_AWAKE = '1'
    VICTORY_SHOW_AFTER = '1'; VICTORY_AMBIENT = '0.8'; VICTORY_D3D12_DRED = 'light'; VICTORY_SKIP_PIPELINES = 'none'
    VICTORY_THIN = '1'; VICTORY_JOIN_PARTS = '8'; VICTORY_UPLOAD_AHEAD = '1'
    VICTORY_PIPELINE_WAIT_MS = '10000'; VICTORY_DYNAMIC_SLOTS = '524288'; VICTORY_OWN_UPSCALER = '1'
    VICTORY_BVH_MISS = '1'; VICTORY_CARD_BUFFERS_MIN_MB = '1'; VICTORY_CARD_BUFFERS_MB = '1536'; VICTORY_COMPOSE = '1'
    VICTORY_F16_NEAREST = '1'
}
if ($HighRam) { $settings.VICTORY_CARD_ALL = '1'; $settings.VICTORY_FETCH_CHECK = '1' }
if ($HalfScale) { $settings.VICTORY_RENDER_SCALE = '0.5' }
# 2026-11-26: Loydben's 0a58ad20 order runs 20 computes in an approximate form (rainbow noise on patterned cloth and
# tattoos); these 22 digest prefixes (his two + those 20) take the paired form first. The water volumetrics
# (581dab2c...) is not among them: it draws dark slabs in every form and stays dropped. Fps panel on (F3).
$settings.VICTORY_PAIRS_FIRST = '1'
$settings.VICTORY_PAIRED_ONLY = 'b5b838548eaa4050638308fb2c805641,2431bbe7dc154ad759d20bc954829c1c,eb1c062cf6b4,ee48bcb1aac8,0fc5f6c65590,4179943de444,96145550a0c6,92328756091f,bb07e51de3b9,2b53ca687fa9,919fa8da66a6,f2dd09fc9ee8,20cbd0dbb1fd,89a7c0c9801f,cba79ed7e1b4,40b353b4a749,87f6d16d1b30,39d2983b3f1a,dd607f0436e0,489fa6032f23,e387b84fdc0c,68bf77351adc'
$settings.VICTORY_PANEL = '1'
# Enter through the start screens (their -AutoStart, carried on to five minutes).
$settings.VICTORY_KEYS = ((25..295 | Where-Object { $_ % 10 -eq 5 }) | ForEach-Object { "${_}:Enter" }) -join ','
# Ours: where the files are, and the log's level.
$settings.VICTORY_LOG = 'info'
$settings.VICTORY_CACHE_DIR = Join-Path $Data 'cache-highram'
$settings.VICTORY_CONFIG = 'C:\Users\Corgan\Desktop\victory-corgi\victory.toml'
foreach ($k in $More.Keys) { $settings[$k] = $More[$k] }

Get-ChildItem Env:VICTORY_* | ForEach-Object { Remove-Item "Env:$($_.Name)" }
foreach ($k in $settings.Keys) { Set-Item "Env:$k" $settings[$k] }
New-Item -ItemType Directory -Force -Path $settings.VICTORY_CACHE_DIR, (Join-Path $Data 'runs') | Out-Null

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$log = Join-Path $Data "runs\$stamp-highram.log"
$console = if ($SeriesX) { 'series-x' } else { 'series-s' }
$line = "`"$victory`" run `"$Exe`" --execute --console $console > `"$log`" 2>&1"
Start-Process -WindowStyle Hidden -WorkingDirectory $Repo cmd -ArgumentList '/c', "`"$line`""
# The window to watch: the run's log as it is written.
$watch = "`$host.UI.RawUI.WindowTitle = 'VIctory HighRAM log - $stamp'; Get-Content -LiteralPath '$log' -Wait -Tail 80"
Start-Process powershell -ArgumentList '-NoLogo', '-NoProfile', '-NoExit', '-ExecutionPolicy', 'Bypass', '-Command', $watch
Start-Sleep -Seconds 3
$p = Get-Process victory -ErrorAction SilentlyContinue | Select-Object -First 1
Write-Output "log $log"
Write-Output ("pid " + $(if ($p) { $p.Id } else { 'none: look at the log' }))
Write-Output ("console $console, HighRam $HighRam, settings: " + (($settings.Keys | Where-Object { $_ -ne 'VICTORY_KEYS' } | Sort-Object | ForEach-Object { "$_=$($settings[$_])" }) -join ' '))
