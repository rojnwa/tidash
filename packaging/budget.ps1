# The performance budget (spec §10) on Windows, after `cargo build --release -p tidash`:
#   pwsh packaging/budget.ps1 target\release\tidash.exe
# macOS and Linux have budget.sh.
#
# It starts Tidash on the user's own profile, so log in first and quit any running Tidash. It waits
# 15 s for Home, measures 10 s of idle, ends Tidash and prints:
# - first frame: start to the first drawn frame (target < 500 ms with a warm cache; the first launch
#   of a new build is slower, so run it again if that one misses);
# - RAM after Home: the working set (target < 150 MB, as spec §10 measures Windows), and private
#   bytes beside it;
# - idle passes in 10 s: frames drawn while nothing changes (target 0);
# - CPU idle: CPU time over those 10 s, in % of one core (no target of its own; expect ~0 %);
# - binary: the executable's size (the 20 MB target is the installer's).
# It exits 1 if a target is missed, 2 if Tidash ended early. Still by hand, as docs/releasing.md
# lists: CPU paused with the window hidden (0 %), CPU playing with the window minimized (< 1 %), and
# scroll pacing (PresentMon).
param([Parameter(Mandatory)][string]$Exe)
$ErrorActionPreference = 'Stop'

function F1([double]$x) { $x.ToString('F1', [cultureinfo]::InvariantCulture) }
function Passes {
    @(Get-Content -LiteralPath $log | Select-String -SimpleMatch 'repaint causes').Count
}

$Exe = (Resolve-Path -LiteralPath $Exe).Path
$log = (New-TemporaryFile).FullName
$env:TIDASH_REPAINT_LOG = '1'
$env:RUST_LOG = 'tidash=info'
$app = Start-Process -FilePath $Exe -RedirectStandardError $log -PassThru
try {
    Start-Sleep -Seconds 15
    if ($app.HasExited) {
        [Console]::Error.WriteLine('budget.ps1: Tidash ended early; is another Tidash running?')
        exit 2
    }
    # A Process object loads its memory figures on first use, so read them now.
    $atHome = Get-Process -Id $app.Id
    $ram = $atHome.WorkingSet64 / 1MB
    $privateBytes = $atHome.PrivateMemorySize64 / 1MB
    $passes1 = Passes
    $cpu1 = $atHome.TotalProcessorTime
    Start-Sleep -Seconds 10
    $passes2 = Passes
    $cpu2 = (Get-Process -Id $app.Id).TotalProcessorTime
    $first = Get-Content -LiteralPath $log | Select-String 'first frame (\d+) ms after start' |
        Select-Object -First 1
} finally {
    Stop-Process -Id $app.Id -ErrorAction SilentlyContinue
    $app.WaitForExit()
    Remove-Item -LiteralPath $log -ErrorAction SilentlyContinue
}

$first = if ($first) { [int]$first.Matches[0].Groups[1].Value } else { $null }
$idle = $passes2 - $passes1
$cpu = ($cpu2 - $cpu1).TotalSeconds * 10
# 10^6 bytes, as spec §10 counts binary sizes; RAM is in MiB.
$size = (Get-Item -LiteralPath $Exe).Length / 1e6

if ($null -eq $first) { 'first frame: none' } else { "first frame: $first ms" }
"RAM after Home: $(F1 $ram) MB ($(F1 $privateBytes) MB private)"
"idle passes in 10 s: $idle"
"CPU idle: $(F1 $cpu) % of one core"
"binary: $(F1 $size) MB"

$missed = @()
if ($null -eq $first -or $first -ge 500) { $missed += 'first frame' }
if ($ram -ge 150) { $missed += 'RAM' }
if ($idle -gt 0) { $missed += 'idle passes' }
if ($missed) {
    [Console]::Error.WriteLine("missed: $($missed -join ', ')")
    exit 1
}
