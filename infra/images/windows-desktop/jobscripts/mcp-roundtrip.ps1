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

$requests = @(
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"gpu-smoke","version":"1"}}}'
  '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"project_new","arguments":{}}}'
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"timeline_get_state","arguments":{}}}'
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
foreach ($id in 1, 2, 3) {
  if (-not $seen.ContainsKey($id)) { throw "no answer to request $id" }
  if ($seen[$id].error) { throw "request $id failed: $($seen[$id].error | ConvertTo-Json -Compress)" }
}
if ($seen[2].result.isError) { throw "project.new answered a tool error: $($seen[2].result | ConvertTo-Json -Depth 6 -Compress)" }
if ($seen[3].result.isError) { throw "timeline.get_state answered a tool error: $($seen[3].result | ConvertTo-Json -Depth 6 -Compress)" }
'project.new and timeline.get_state both round-tripped through the editor on screen'
