# Runs in the auto-logon user's console session, sent there by the image's
# interactive helper (TASK-138). Starts the editor from the Start Menu entry
# the MSI installs and waits for a real window.
$ErrorActionPreference = 'Stop'

# Resolve the shortcut rather than hard-coding the exe: this is what a person
# double-clicks, so it is what the job should exercise.
$lnk = 'C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Subordinate\Subordinate.lnk'
if (-not (Test-Path $lnk)) { throw "no Start Menu shortcut at $lnk" }
$shell = New-Object -ComObject WScript.Shell
$sc = $shell.CreateShortcut($lnk)
"shortcut -> $($sc.TargetPath)"

$project = 'C:\SubordinateTest\sample-project\demo.sub'
if (-not (Test-Path $project)) { throw "no sample project at $project" }

$out = 'C:\SubordinateTest\app.out'
$err = 'C:\SubordinateTest\app.err'
Remove-Item $out, $err, C:\SubordinateTest\app.pid -Force -ErrorAction SilentlyContinue

# Win32_Process.Create, not Start-Process. The helper runs this script as
# `powershell.exe ... *> log`, and anything started with Start-Process inherits
# those handles: the redirection does not close until every inheritor exits, so
# the helper would sit waiting for the editor - which is meant to stay open -
# and the job would time out with the editor happily running (run 34669759392).
# Win32_Process.Create detaches completely, in this same console session, and
# cmd does the redirection itself.
$exe = $sc.TargetPath
$command = 'cmd.exe /c ""' + $exe + '" "' + $project + '" > "' + $out + '" 2> "' + $err + '""'
$created = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{
  CommandLine      = $command
  CurrentDirectory = $sc.WorkingDirectory
}
if ($created.ReturnValue -ne 0) { throw "Win32_Process.Create returned $($created.ReturnValue)" }
"launcher pid=$($created.ProcessId)"

$deadline = (Get-Date).AddSeconds(120)
$app = $null
while ((Get-Date) -lt $deadline) {
  $app = Get-Process -Name subordinate -ErrorAction SilentlyContinue |
         Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
  if ($app) { break }
  Start-Sleep -Milliseconds 500
}
if (-not $app) {
  'no window yet; last lines of the editor log:'
  Get-Content $err -ErrorAction SilentlyContinue | Select-Object -Last 40
  throw 'the editor showed no window within 120s'
}
"pid=$($app.Id) window handle=$($app.MainWindowHandle) title=$($app.MainWindowTitle)"
$app.Id | Set-Content -Encoding ASCII C:\SubordinateTest\app.pid

# A few seconds of frames so the project is drawn before anything is captured.
Start-Sleep 8
Get-Content $err -ErrorAction SilentlyContinue | Select-Object -Last 20
