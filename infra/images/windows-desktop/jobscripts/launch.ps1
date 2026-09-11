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

$stdout = 'C:\SubordinateTest\app.out'
$stderr = 'C:\SubordinateTest\app.err'
$proc = Start-Process -PassThru -FilePath $sc.TargetPath -ArgumentList $project `
  -WorkingDirectory $sc.WorkingDirectory -RedirectStandardOutput $stdout -RedirectStandardError $stderr
"pid=$($proc.Id)"
$proc.Id | Set-Content -Encoding ASCII C:\SubordinateTest\app.pid

$deadline = (Get-Date).AddSeconds(120)
while ((Get-Date) -lt $deadline) {
  $proc.Refresh()
  if ($proc.HasExited) {
    Get-Content $stderr -ErrorAction SilentlyContinue | Select-Object -Last 40
    throw "the editor exited with $($proc.ExitCode) before it showed a window"
  }
  if ($proc.MainWindowHandle -ne 0) { break }
  Start-Sleep -Milliseconds 500
}
if ($proc.MainWindowHandle -eq 0) {
  Get-Content $stderr -ErrorAction SilentlyContinue | Select-Object -Last 40
  throw 'the editor showed no window within 120s'
}
"window handle=$($proc.MainWindowHandle) title=$($proc.MainWindowTitle)"

# A few seconds of frames so the project is drawn before anything is captured.
Start-Sleep 8
Get-Content $stderr -ErrorAction SilentlyContinue | Select-Object -Last 20
