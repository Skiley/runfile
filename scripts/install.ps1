$ErrorActionPreference = 'Stop'

$repo = 'Skiley/runfile'
$installDir = if ($env:RUNFILE_INSTALL_DIR) { $env:RUNFILE_INSTALL_DIR } else { "$env:LOCALAPPDATA\runfile\bin" }
$version = if ($args[0]) { $args[0] } else { 'latest' }

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
  Move-Item -Path (Join-Path $tmp "runfile-cli-$target\run.exe") -Destination (Join-Path $installDir 'run.exe') -Force

  Write-Host "Installed run.exe to $installDir\run.exe"

  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (-not ($userPath -split ';' -contains $installDir)) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$installDir", 'User')
    Write-Host ""
    Write-Host "Added $installDir to your PATH (open a new shell to use)."
  }
} finally {
  Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
