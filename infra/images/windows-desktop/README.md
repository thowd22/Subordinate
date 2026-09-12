# Windows NVIDIA desktop runner AMI

The image behind the `gpu-nvidia-desktop-windows` runner (TASK-138). It is
RunsOn's stock `windows22-full-x64` with four things added: the NVIDIA driver,
an **auto-logon console session**, the released Subordinate MSI with Python and
pywinauto, and the 4K test clip.

Everything here is committed so the AMI can be rebuilt from scratch; nothing
here contains a secret.

## Why an image at all

`gpu-nvidia-windows` (TASK-115) already runs GPU jobs on the stock image. What
it cannot do is drive the real application, because of where RunsOn puts the
runner. On a Windows instance RunsOn's user data starts the agent directly:

```
bootstrap-<tag>-agent-windows-AMD64.exe   session 0
  cmd.exe                                 session 0
    Runner.Listener.exe                   session 0
      Runner.Worker.exe                   session 0
        pwsh.exe  (your step)             session 0, NT AUTHORITY\SYSTEM
```

(measured on run 34655612366). Session 0 is Windows' service session: it has no
desktop, shows no window and receives no input. In that session the editor does
not merely look wrong, it dies - run 34656029109 caught it:

```
ERROR wgpu_hal::dx12: SwapChain creation error: ... (0x887A0022)
thread 'main' panicked ... In Surface::configure / Invalid surface
```

### Why the agent is not moved into the desktop session

The obvious fix is to start the GitHub Actions runner from a logon task of an
auto-logon user instead. It is not available to us: the agent is **not** a
Windows service that an image can reconfigure. It is a child of the instance's
user data, which belongs to the RunsOn control plane, is regenerated per job
(it carries that job's JIT runner registration) and is not something a custom
AMI can edit. The only thing an AMI could do is replace
`C:\runs-on\bootstrap-<tag>.exe` with a shim - overwriting a binary the vendor
owns, at a path its version tag pins, which would break silently on the next
RunsOn upgrade and is explicitly not how RunsOn documents custom images
(https://runs-on.com/docs/runners/custom/: keep `C:\actions-runner` and
`C:\runs-on\bootstrap-TAG.exe` as they are).

So the image keeps RunsOn's arrangement exactly and adds a bridge instead.

## The interactive helper

The image logs `subtest` (a local administrator) on to the console session at
boot, and a logon-triggered scheduled task, `SubordinateInteractiveHelper`,
starts a small PowerShell loop in that session. A job in session 0 talks to it
through a directory:

| Path | What it is |
| --- | --- |
| `C:\SubordinateTest\queue\<id>.ps1` | a script the job wants run on the desktop |
| `C:\SubordinateTest\out\<id>.log` | its stdout and stderr |
| `C:\SubordinateTest\out\<id>.exit` | its exit code, written **last** |
| `C:\SubordinateTest\helper.json` | heartbeat: pid, session id, user, desktop size, timestamp |

`C:\SubordinateTest\bin\InteractiveSession.psm1` wraps that protocol:

```powershell
Import-Module C:\SubordinateTest\bin\InteractiveSession.psm1
$r = Invoke-InteractiveScript -Name 'launch' -TimeoutSeconds 240 -Script (Get-Content .\launch.ps1 -Raw)
$r.Output; if ($r.ExitCode -ne 0) { throw 'failed' }
```

`Invoke-InteractiveScript` refuses to run when there is no heartbeat, when the
heartbeat is more than 30 s old, or when it says session 0 - so "the auto-logon
did not happen" is a clear error rather than a mystery timeout.

The queued script runs with no arguments and no inherited environment. Pass a
value by prepending an assignment, as the workflow does for `$ShotName`.

`jobscripts/` holds the scripts the GPU smoke job sends through that bridge:
`launch.ps1`, `screenshot.ps1`, `import-click.ps1` with `import-click.py`,
`mcp-roundtrip.ps1` and `finish.ps1`.

## What the image contains

| Component | Step | What it does |
| --- | --- | --- |
| `component-1-gpu.yml` | AWS CLI, NVIDIA driver, reboot, `nvidia-smi` | driver from the anonymously readable `s3://ec2-windows-nvidia-drivers`, installed `-s -n` |
| `component-2-session.yml` | test user, auto-logon, no lock or screensaver, RDP | `subtest` with a generated password, `AutoAdminLogon`, `DisableLockWorkstation`, no sleep, RDP on |
| `component-3-helper.yml` | interactive helper | the loop, the logon task and `InteractiveSession.psm1` |
| `component-4-payload.yml` | MSI, Python, media, validate | newest `v*` release `.msi`, machine-wide Python 3.12 with pywinauto, `meld-4k60-excerpt-2min.mkv` verified by SHA-256 |

Four components rather than one because `CreateComponent` caps a component
document at 16 000 characters.

`C:\SubordinateTest\image.json` records which release went in, so a job can
print it.

## The password

The build generates a random 32-character password for `subtest`, writes it to
the Secrets Manager secret `subordinate/windows-desktop-ami/rdp`, and writes it
nowhere else. Read it with:

```bash
aws secretsmanager get-secret-value --region us-east-1 \
  --secret-id subordinate/windows-desktop-ami/rdp --query SecretString --output text
```

It is also in the image's `Winlogon\DefaultPassword` registry value in clear,
because that is how Windows auto-logon works, and in
`C:\SubordinateTest\bin\apply-autologon.ps1`, which `SetupComplete.cmd` re-runs
after Sysprep (Sysprep's generalize pass can clear the auto-logon values, and
Image Builder syspreps at the end of every build). **Keep the AMI private to
the account and do not share the snapshot.** A rebuild rotates the password.

To watch a run, RDP to the instance as `subtest`; the smoke job prints the
instance id. The security group RunsOn attaches does not open 3389, so add a
temporary rule from your own address first, and remember the instance dies with
the job. `docs/DEVELOPMENT.md` has the wider self-hosted/RDP notes.

## Rebuilding

```bash
./build.sh 1.0.4          # new version, creates everything and starts a build
./build.sh 1.0.4 --no-run # create/update the resources only
```

Image Builder makes component and recipe versions immutable, so pass a version
that has not been used. The script is otherwise idempotent: it creates or
updates the instance profile, the security group, the infrastructure and
distribution configurations and the pipeline.

Watch it, and get the AMI id out:

```bash
arn=arn:aws:imagebuilder:us-east-1:731537225673:image/subordinate-windows-desktop/1.0.4/1
aws imagebuilder get-image --region us-east-1 --image-build-version-arn "$arn" \
  --output text --query 'image.state.status'
aws imagebuilder get-image --region us-east-1 --image-build-version-arn "$arn" \
  --output text --query 'image.outputResources.amis[0].image'
```

Build logs land in `s3://subordinate-test-media-731537225673/imagebuilder-logs/`.

Then put the new id in `.github/runs-on.yml` under `gpu-nvidia-desktop-windows`
and merge it to `main` - RunsOn reads that file from the default branch only.
Test from a branch with the inline form instead:
`runs-on=${{ github.run_id }}/ami=ami-xxxx/family=g4dn.xlarge/spot=false`.

**Rebuild at least every 30 days.** GitHub stops dispatching jobs to a runner
whose agent binary is older than 30 days, and the agent in this image is the
one baked into the parent AMI. RunsOn recommends every ~15 days.

Also refresh `PARENT_IMAGE` in `build.sh` when RunsOn publishes a newer stock
Windows image:

```bash
aws ec2 describe-images --region us-east-1 --owners 135269210855 \
  --filters Name=name,Values='runs-on-*-windows22-full-x64-*' \
  --query 'sort_by(Images,&CreationDate)[-1].[ImageId,Name]' --output text
```

## Prerequisites outside this directory

* The test-media bucket policy must let the build instance profile read the
  clip and write build logs. `iam-inline-policy.json` is this role's half of
  it; the bucket's half is two statements naming
  `arn:aws:iam::731537225673:role/SubordinateWindowsDesktopImageBuilder`
  (`s3:GetObject` + `s3:ListBucket` on the bucket, `s3:PutObject` under
  `imagebuilder-logs/*`).
* EC2 G-family on-demand vCPU quota above zero in us-east-1 (TASK-113): the
  build runs on a `g4dn.xlarge` so the driver installs against a real T4.

## Cost

A build is one on-demand `g4dn.xlarge` Windows instance (about 0.75 USD/h) for
the length of the build, plus the snapshot (about 30 GB of the 80 GB volume at
0.05 USD/GB-month). A job on the finished image is the same instance type for
the length of the job. Measured numbers are in `docs/DEVELOPMENT.md` under
"GPU CI (RunsOn)".
