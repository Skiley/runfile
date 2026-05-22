$ErrorActionPreference = 'Stop'
# Invoke-WebRequest renders a progress bar that is extremely slow when stdout is
# redirected (i.e. run non-interactively, as `run :update` does) on Windows
# PowerShell 5.1 — suppress it so the download doesn't appear to hang.
$ProgressPreference = 'SilentlyContinue'

$repo = 'Skiley/runfile'
$installDir = if ($env:RUNFILE_INSTALL_DIR) { $env:RUNFILE_INSTALL_DIR } else { "$env:LOCALAPPDATA\runfile\bin" }
# Version precedence: positional arg, then $env:RUNFILE_VERSION (how `run
# :update` pins a release — the `iwr ... | iex` invocation form can't pass
# positional args), then latest.
$version = if ($args[0]) { $args[0] } elseif ($env:RUNFILE_VERSION) { $env:RUNFILE_VERSION } else { 'latest' }

$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
  'AMD64' { 'x86_64' }
  'ARM64' { 'aarch64' }
  default { throw "runfile: unsupported architecture: $env:PROCESSOR_ARCHITECTURE" }
}

$target = "$arch-pc-windows-msvc"
$archive = "runfile-cli-$target.zip"

$url = if ($version -eq 'latest') {
  "https://github.com/$repo/releases/latest/download/$archive"
} else {
  "https://github.com/$repo/releases/download/$version/$archive"
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ([guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

try {
  Write-Host "Downloading $archive..."
  Invoke-WebRequest -Uri $url -OutFile (Join-Path $tmp $archive) -UseBasicParsing
  Expand-Archive -Path (Join-Path $tmp $archive) -DestinationPath $tmp

  New-Item -ItemType Directory -Path $installDir -Force | Out-Null
  $dest = Join-Path $installDir 'run.exe'

  # Windows won't let you overwrite a running .exe, but it WILL let you rename
  # one (renaming only touches the directory entry, not the locked file data).
  # So move any existing binary aside before dropping the new one in. We rename
  # to a UNIQUE name rather than a fixed `run.exe.old`: a previous update may
  # have left a `.old` that is still locked (its process hadn't exited, or an
  # AV/indexer held a handle), and `Rename-Item` cannot overwrite an existing
  # name — it would fail and leave run.exe in place, after which `Move-Item`
  # can't overwrite the running binary either. A fresh GUID-suffixed name can
  # never collide, so the rename — and therefore the whole update — always
  # succeeds.
  if (Test-Path $dest) {
    $asideName = "run.exe.old-$([guid]::NewGuid().ToString('N'))"
    Rename-Item -Path $dest -NewName $asideName
  }
  Move-Item -Path (Join-Path $tmp "runfile-cli-$target\run.exe") -Destination $dest -Force

  # Sweep aside-files from this and prior updates. The one we just created is
  # the live process image during `run :update` and stays locked (its delete
  # fails and is ignored); ones left by finished prior updates are dead files
  # and get cleaned up here.
  Get-ChildItem -Path $installDir -Filter 'run.exe.old*' -ErrorAction SilentlyContinue |
    ForEach-Object { Remove-Item -Path $_.FullName -Force -ErrorAction SilentlyContinue }

  Write-Host "Installed run.exe to $dest"

  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (-not ($userPath -split ';' -contains $installDir)) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$installDir", 'User')
    Write-Host ""
    Write-Host "Added $installDir to your PATH (open a new shell to use)."
  }
} finally {
  Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
