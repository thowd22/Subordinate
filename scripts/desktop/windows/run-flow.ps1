# Run one desktop flow inside the auto-logon console session (TASK-139).
#
# The GitHub Actions runner on the Windows desktop image is SYSTEM in session
# 0, which has no desktop: nothing there can open a window, click in one or
# photograph it. So a job sends this script into the console session through
# C:\SubordinateTest\bin\InteractiveSession.psm1, and it is here - as the
# logged-on user, with a real desktop - that Python, pywinauto and the editor
# meet.
#
# The helper runs a queued script with no arguments and no inherited
# environment, so the caller prepends assignments:
#
#   $FlowScript = 'C:\SubordinateTest\desktop\flow_mcp.py'
#   $FlowEnv    = '{"FLOW_OUT":"C:\\SubordinateTest\\flow-out"}'
#
# Everything the flow prints comes back in the helper's log, and its exit code
# is the flow's, so a failing step fails the job.
$ErrorActionPreference = 'Stop'

if (-not $FlowScript) { throw 'no $FlowScript: prepend an assignment naming the flow to run' }
if (-not (Test-Path $FlowScript)) { throw "no flow script at $FlowScript" }

$python = 'C:\Program Files\Python312\python.exe'
if (-not (Test-Path $python)) { throw "no machine-wide python at $python" }

if ($FlowEnv) {
  $settings = $FlowEnv | ConvertFrom-Json
  foreach ($name in $settings.PSObject.Properties.Name) {
    $value = [string]$settings.$name
    Set-Item -Path "env:$name" -Value $value
    "env $name=$value"
  }
}

"python:  $python"
"flow:    $FlowScript"
"session: $((Get-Process -Id $PID).SessionId) as $env:USERNAME"

# -u so the step's log carries the flow's own step lines as they happen rather
# than in one block at the end - a flow that hangs should still say where.
& $python -u $FlowScript
$code = $LASTEXITCODE
"flow exit=$code"
exit $code
