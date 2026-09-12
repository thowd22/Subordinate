# Proves an MCP round-trip against the editor that is on screen (TASK-138).
#
# subordinate-mcp is a stdio bridge: it finds the running editor through the
# lock file the Command API endpoint publishes under the *user's*
# LOCALAPPDATA, and talks to it over a named pipe. That is why this runs in
# the console session rather than in the job: only there is it the same user
# as the editor.
$ErrorActionPreference = 'Stop'

# The MSI does not ship the bridge yet (checked on v0.1.1: build-msi.ps1 stages
# subordinate.exe and subordinate-cli.exe only), so fall back to the copy the
# workflow's free hosted-runner job built and staged.
$bridge = 'C:\Program Files\Subordinate\bin\subordinate-mcp.exe'
if (-not (Test-Path $bridge)) { $bridge = 'C:\SubordinateTest\bin\subordinate-mcp.exe' }
if (-not (Test-Path $bridge)) { throw 'no subordinate-mcp.exe anywhere' }
"bridge: $bridge"

$lock = "$env:LOCALAPPDATA\Subordinate\run\default.lock.json"
if (-not (Test-Path $lock)) { throw "no Command API lock file at $lock - is the editor running?" }
Get-Content $lock

# Never let the bridge start a headless engine of its own: this has to be the
# window on screen answering, not a server the bridge launched for itself.
$env:SUBORDINATE_MCP_NO_LAUNCH = '1'

# project.new leaves an empty project, and timeline.get_state on a project with
# no sequences - or with the two that demo.sub carries - answers a domain error
# rather than a timeline, so the round trip creates one sequence in between.
$settings = '{"resolution":{"width":1920,"height":1080},"frame_rate":{"numerator":30,"denominator":1},"sample_rate":48000,"color":{"space":"rec709","transfer":"bt709","primaries":"bt709"}}'
$requests = @(
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"gpu-smoke","version":"1"}}}'
  '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"project_new","arguments":{"name":"gpu-smoke"}}}'
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"sequence_create","arguments":{"name":"Smoke","settings":' + $settings + '}}}'
  '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"timeline_get_state","arguments":{}}}'
)
Set-Content -Encoding ASCII C:\SubordinateTest\mcp-in.jsonl ($requests -join "`r`n")

# Piping through cmd keeps stdin, stdout and stderr straight without
# Start-Process redirection games; the bridge exits when stdin reaches EOF.
cmd /c "type C:\SubordinateTest\mcp-in.jsonl | ""$bridge"" > C:\SubordinateTest\mcp-out.jsonl 2> C:\SubordinateTest\mcp-err.txt"

'--- subordinate-mcp stderr ---'
Get-Content C:\SubordinateTest\mcp-err.txt -ErrorAction SilentlyContinue | Select-Object -Last 20
'--- subordinate-mcp stdout ---'
$lines = @(Get-Content C:\SubordinateTest\mcp-out.jsonl -ErrorAction SilentlyContinue)
foreach ($line in $lines) {
  if ($line.Length -gt 600) { $line.Substring(0, 600) + ' ...' } else { $line }
}

$seen = @{}
foreach ($line in $lines) {
  if (-not $line.Trim()) { continue }
  $message = $line | ConvertFrom-Json
  if ($null -ne $message.id) { $seen[[int]$message.id] = $message }
}
$named = @{ 1 = 'initialize'; 2 = 'project.new'; 3 = 'sequence.create'; 4 = 'timeline.get_state' }
foreach ($id in 1, 2, 3, 4) {
  if (-not $seen.ContainsKey($id)) { throw "no answer to $($named[$id]) (request $id)" }
  if ($seen[$id].error) { throw "$($named[$id]) failed: $($seen[$id].error | ConvertTo-Json -Compress)" }
  if ($seen[$id].result.isError) {
    throw "$($named[$id]) answered a tool error: $($seen[$id].result | ConvertTo-Json -Depth 6 -Compress)"
  }
}
'project.new, sequence.create and timeline.get_state all round-tripped through the editor on screen'
