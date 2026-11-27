# Starts the title on the VIctory-NVIDIA-HighRAM build (Loydben/VIctory-NVIDIA-HighRAM) with the settings of
# its own scripts\play-highram.ps1, a run log in D:\victory-data\runs and a console window that follows the log.
#   run-highram.ps1                 their defaults: Series S, VICTORY_CARD_ALL and VICTORY_FETCH_CHECK on
#   run-highram.ps1 -HighRam:$false without those two (their advice for smaller cards)
#   run-highram.ps1 -SeriesX        the Series X profile
#   run-highram.ps1 -HalfScale      the world at half the Series S size
#   run-highram.ps1 -RT             the title's ray tracing: its structure builder on, ray walks not cut at the
#                                   root, and the builder's three wave64 programs in the paired form (bounce light;
#                                   12-16 fps on the boat against 21-23). Its shader cache is apart from the plain one.
#   run-highram.ps1 -More @{VICTORY_X='1'}
#   run-highram.ps1 -NoMenu         without the live settings window (sliders; the game's own overlay is F8 either way)
# Nothing of the game is written below the source tree: the cache is D:\victory-data\cache-highram.
param(
    [bool]$HighRam = $true,
    [switch]$SeriesX,
    [switch]$HalfScale,
    [switch]$RT,
    [switch]$NoMenu,
    [hashtable]$More = @{},
    [string]$Repo = 'C:\Users\Corgan\Desktop\VIctory-NVIDIA-HighRAM-main',
    # 2026-11-26: the low-spec copy (scarlett_lowspec.py: 420p render through the title's own upscaler) by default.
    [string]$Exe = 'D:\GTA 6 Xbox Build\GTA6.Xbox.v1.0.2.1-CYBERLEEK\game_scarlett_master_lowspec.exe',
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
# tattoos); these 22 digest prefixes (his two + those 20) take the paired form first. Plus the water's four
# (2026-11-26 13:15): the tile classifier 581dab2c and the three passes that use its tile lists (50cf2832,
# 24126c14, 7099a57f). The classifier alone drew dark slabs (its tiles listed, their passes refused); all
# four out left the water dark navy. All four in: turquoise water. Fps panel on (F3).
$settings.VICTORY_PAIRS_FIRST = '1'
$settings.VICTORY_PAIRED_ONLY = 'b5b838548eaa4050638308fb2c805641,2431bbe7dc154ad759d20bc954829c1c,eb1c062cf6b4,ee48bcb1aac8,0fc5f6c65590,4179943de444,96145550a0c6,92328756091f,bb07e51de3b9,2b53ca687fa9,919fa8da66a6,f2dd09fc9ee8,20cbd0dbb1fd,89a7c0c9801f,cba79ed7e1b4,40b353b4a749,87f6d16d1b30,39d2983b3f1a,dd607f0436e0,489fa6032f23,e387b84fdc0c,68bf77351adc,581dab2cd8bc,50cf28328b58,24126c14dc76,7099a57fded1'
$settings.VICTORY_PANEL = '1'
# 2026-11-26 13:20: windows outside buffers in card memory the CPU maps (up to 4 GB): the card read them from the
# PC's memory over the bus every frame. A/B, same save and keys: none 21.2 fps, 2 GB 22.3, 4 GB 22.1-23.2. The card's
# budget stops them at about 2.4 GB (VRAM full: buffer copies 2.5 GB, textures 2.2, windows, targets 1.2).
$settings.VICTORY_WINDOWS_BAR_MB = '4096'
# 2026-11-26 14:03: card copies of title buffers up to 3 GB (theirs 1.5): same A/B, 1 GB 22.6 fps, 1.5 GB 22.2, 3 GB 23.4.
$settings.VICTORY_CARD_BUFFERS_MB = '3072'
# Enter through the start screens (their -AutoStart, carried on to five minutes).
$settings.VICTORY_KEYS = ((25..295 | Where-Object { $_ % 10 -eq 5 }) | ForEach-Object { "${_}:Enter" }) -join ','
# Ours: where the files are, and the log's level.
$settings.VICTORY_LOG = 'info'
$settings.VICTORY_CACHE_DIR = Join-Path $Data 'cache-highram'
$settings.VICTORY_CONFIG = 'C:\Users\Corgan\Desktop\victory-corgi\victory.toml'
# 2026-11-26: settings changed while the game runs (crates\victory-gpu\src\live.rs): the slider window
# (scripts\live-menu.ps1) and the overlay in the game (F8) write this file; victory.exe reads it again when it changes.
$settings.VICTORY_LIVE_FILE = Join-Path $Data 'live-settings.txt'
if ($RT) {
    # 2026-11-26: the builder stays off unless all 53 of its programs have a host pipeline; 39321a95, 77360df2 and
    # 9fc111d3 (DXBC digests below) have one only in the paired form. Empty unsets the switch (Set-Item Env:).
    $settings.VICTORY_BVH_BUILDER = '1'
    $settings.VICTORY_BVH_MISS = ''
    $rtPaired = (Get-Content 'D:\victory-data\snap\paired-only-rt.txt' -ErrorAction SilentlyContinue | Select-Object -First 1)
    if ($rtPaired) { $settings.VICTORY_PAIRED_ONLY = $rtPaired.Trim() }
}
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
# The live settings window, once (it follows each new run by itself).
$liveMenu = Join-Path $Repo 'scripts\live-menu.ps1'
if (-not $NoMenu -and $settings.VICTORY_LIVE_FILE -and (Test-Path $liveMenu)) {
    Remove-Item -LiteralPath "$($settings.VICTORY_LIVE_FILE).render" -ErrorAction SilentlyContinue
    $open = Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" | Where-Object { $_.CommandLine -like '*live-menu.ps1*' }
    if (-not $open) {
        Start-Process powershell -WindowStyle Hidden -ArgumentList '-NoLogo', '-NoProfile', '-STA', '-ExecutionPolicy', 'Bypass', '-File', "`"$liveMenu`"", '-Settings', "`"$($settings.VICTORY_LIVE_FILE)`""
    }
}
Start-Sleep -Seconds 3
$p = Get-Process victory -ErrorAction SilentlyContinue | Select-Object -First 1
Write-Output "log $log"
Write-Output ("pid " + $(if ($p) { $p.Id } else { 'none: look at the log' }))
Write-Output ("console $console, HighRam $HighRam, settings: " + (($settings.Keys | Where-Object { $_ -ne 'VICTORY_KEYS' } | Sort-Object | ForEach-Object { "$_=$($settings[$_])" }) -join ' '))
