param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9]+\.[0-9]+\.[0-9]+$')]
    [string]$Version,
    [string]$BinaryPath = "target/x86_64-pc-windows-msvc/release/explorer.exe",
    [string]$OutputDir = "dist/squirrel",
    [string]$NuGetExe = "nuget",
    [string]$SquirrelExe = "",
    [string]$SquirrelWindowsVersion = "2.0.1"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-RepoRoot {
    return (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\..")).Path
}

function Resolve-PathUnderRepo {
    param([string]$RepoRoot, [string]$Path, [string]$Label)
    $candidate = if ([System.IO.Path]::IsPathRooted($Path)) {
        [System.IO.Path]::GetFullPath($Path)
    } else {
        [System.IO.Path]::GetFullPath((Join-Path $RepoRoot $Path))
    }
    $rootWithSeparator = $RepoRoot.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
    if (-not $candidate.StartsWith($rootWithSeparator, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "$Label must remain inside the repository: $candidate"
    }
    if ($candidate -eq $RepoRoot) {
        throw "$Label must not be the repository root."
    }
    return $candidate
}

function Remove-GuardedDirectory {
    param([string]$RepoRoot, [string]$Path, [string]$Label)
    $guarded = Resolve-PathUnderRepo -RepoRoot $RepoRoot -Path $Path -Label $Label
    if (Test-Path -LiteralPath $guarded) {
        $item = Get-Item -LiteralPath $guarded
        if (-not $item.PSIsContainer) {
            throw "$Label cleanup target is not a directory: $guarded"
        }
        Remove-Item -LiteralPath $guarded -Recurse -Force
    }
}

function Resolve-CommandPath {
    param([string]$CommandName)
    $command = Get-Command $CommandName -ErrorAction SilentlyContinue
    if (-not $command) {
        throw "Required command '$CommandName' was not found."
    }
    return $command.Source
}

function Resolve-SquirrelExecutable {
    param([string]$ProvidedPath, [string]$NuGetPath, [string]$ToolsDir, [string]$PackageVersion)
    if ($ProvidedPath) {
        if (-not (Test-Path -LiteralPath $ProvidedPath -PathType Leaf)) {
            throw "Provided Squirrel executable does not exist: $ProvidedPath"
        }
        return (Resolve-Path -LiteralPath $ProvidedPath).Path
    }
    New-Item -ItemType Directory -Path $ToolsDir -Force | Out-Null
    & $NuGetPath install Squirrel.Windows -Version $PackageVersion -OutputDirectory $ToolsDir -ExcludeVersion -NonInteractive | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to install Squirrel.Windows $PackageVersion via NuGet."
    }
    $candidate = Join-Path $ToolsDir "Squirrel.Windows\tools\Squirrel.exe"
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
        throw "Squirrel.exe was not installed at the expected path: $candidate"
    }
    return $candidate
}

function Test-BinaryContainsAsciiText {
    param([string]$Path, [string]$Text)
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $pattern = [System.Text.Encoding]::ASCII.GetBytes($Text)
    for ($offset = 0; $offset -le $bytes.Length - $pattern.Length; $offset++) {
        $matches = $true
        for ($index = 0; $index -lt $pattern.Length; $index++) {
            if ($bytes[$offset + $index] -ne $pattern[$index]) {
                $matches = $false
                break
            }
        }
        if ($matches) { return $true }
    }
    return $false
}

function Assert-NoDummyMarkers {
    param([string]$Path)
    foreach ($marker in @("This is a dummy update,exe", "This is a dummy update.exe")) {
        if (Test-BinaryContainsAsciiText -Path $Path -Text $marker) {
            throw "Detected dummy Squirrel marker in '$Path': '$marker'."
        }
    }
}

function Get-StreamSha256 {
    param([System.IO.Stream]$Stream)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([System.BitConverter]::ToString($sha.ComputeHash($Stream))).Replace("-", "")
    } finally {
        $sha.Dispose()
    }
}

function Assert-NuGetPackage {
    param([string]$PackagePath, [string]$ExpectedVersion, [string]$BinaryPath, [string]$IconPath, [string]$PackageIconPath)
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($PackagePath)
    try {
        $nuspec = @($archive.Entries | Where-Object { $_.FullName -match '^[^/]+\.nuspec$' })
        if ($nuspec.Count -ne 1) { throw "Package must contain exactly one root nuspec." }
        $reader = New-Object System.IO.StreamReader($nuspec[0].Open())
        try { [xml]$manifest = $reader.ReadToEnd() } finally { $reader.Dispose() }
        if ($manifest.package.metadata.id -ne "explorer") { throw "Package ID must be explorer." }
        if ($manifest.package.metadata.title -ne "Explorer") { throw "Package title must be Explorer." }
        if ($manifest.package.metadata.authors -ne "Harry Merritt") { throw "Package publisher must be Harry Merritt." }
        if ($manifest.package.metadata.owners -ne "Harry Merritt") { throw "Package owner must be Harry Merritt." }
        if ($manifest.package.metadata.icon -ne "explorer.png") { throw "Package icon must be explorer.png." }
        if ($manifest.package.metadata.version -ne $ExpectedVersion) { throw "Package version does not match $ExpectedVersion." }

        foreach ($expected in @(
            @{ Name = "lib/net45/file-explorer.exe"; Source = $BinaryPath },
            @{ Name = "lib/net45/app.ico"; Source = $IconPath },
            @{ Name = "app.ico"; Source = $IconPath },
            @{ Name = "explorer.png"; Source = $PackageIconPath }
        )) {
            $entry = $archive.GetEntry($expected.Name)
            if (-not $entry) { throw "Package is missing $($expected.Name)." }
            $source = Get-Item -LiteralPath $expected.Source
            if ($entry.Length -ne $source.Length) {
                throw "Package size mismatch for $($expected.Name): $($entry.Length) != $($source.Length)."
            }
            $stream = $entry.Open()
            try { $entryHash = Get-StreamSha256 -Stream $stream } finally { $stream.Dispose() }
            $sourceHash = (Get-FileHash -LiteralPath $expected.Source -Algorithm SHA256).Hash
            if ($entryHash -ne $sourceHash) { throw "Package hash mismatch for $($expected.Name)." }
        }
    } finally {
        $archive.Dispose()
    }
}

function Assert-ReleasesManifest {
    param([string]$ReleasesPath, [string]$OutputDir, [string]$ExpectedPackageName)
    $lines = @(Get-Content -LiteralPath $ReleasesPath | Where-Object { $_.Trim() })
    if ($lines.Count -ne 1) { throw "RELEASES must contain exactly one full package entry." }
    if ($lines[0] -notmatch '^([0-9A-Fa-f]{40})\s+(\S+)\s+(\d+)$') {
        throw "RELEASES contains an invalid entry: $($lines[0])"
    }
    $manifestHash = $Matches[1]
    $manifestName = $Matches[2]
    $manifestSize = [int64]$Matches[3]
    if ($manifestName -ne $ExpectedPackageName -or $manifestName -match '-delta\.nupkg$') {
        throw "RELEASES references unexpected package '$manifestName'."
    }
    $packagePath = Join-Path $OutputDir $manifestName
    if (-not (Test-Path -LiteralPath $packagePath -PathType Leaf)) {
        throw "RELEASES references a missing package: $packagePath"
    }
    $package = Get-Item -LiteralPath $packagePath
    if ($package.Length -ne $manifestSize) { throw "RELEASES size does not match $manifestName." }
    $actualHash = (Get-FileHash -LiteralPath $packagePath -Algorithm SHA1).Hash
    if ($actualHash -ne $manifestHash) { throw "RELEASES SHA1 does not match $manifestName." }
}

$repoRoot = Resolve-RepoRoot
$binaryFullPath = Resolve-PathUnderRepo -RepoRoot $repoRoot -Path $BinaryPath -Label "BinaryPath"
$outputFullPath = Resolve-PathUnderRepo -RepoRoot $repoRoot -Path $OutputDir -Label "OutputDir"
$workRoot = Resolve-PathUnderRepo -RepoRoot $repoRoot -Path "dist/squirrel-work" -Label "work directory"
$inputDir = Join-Path $workRoot "input"
$packageDir = Join-Path $workRoot "package"
$toolsDir = Join-Path $workRoot "tools"
$nuspecPath = Join-Path $repoRoot "packaging\windows\squirrel\explorer.nuspec"
$iconPath = Join-Path $repoRoot "assets\explorer.ico"
$packageIconPath = Join-Path $repoRoot "assets\explorer.png"

foreach ($required in @($binaryFullPath, $nuspecPath, $iconPath, $packageIconPath)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) { throw "Required input is missing: $required" }
}

Remove-GuardedDirectory -RepoRoot $repoRoot -Path $workRoot -Label "work directory"
Remove-GuardedDirectory -RepoRoot $repoRoot -Path $outputFullPath -Label "output directory"
New-Item -ItemType Directory -Path $inputDir, $packageDir, $outputFullPath -Force | Out-Null

$nugetPath = Resolve-CommandPath -CommandName $NuGetExe
$squirrelPath = Resolve-SquirrelExecutable -ProvidedPath $SquirrelExe -NuGetPath $nugetPath -ToolsDir $toolsDir -PackageVersion $SquirrelWindowsVersion
Copy-Item -LiteralPath $binaryFullPath -Destination (Join-Path $inputDir "file-explorer.exe")
Copy-Item -LiteralPath $iconPath -Destination (Join-Path $inputDir "app.ico")
Copy-Item -LiteralPath $packageIconPath -Destination (Join-Path $inputDir "explorer.png")

& $nugetPath pack $nuspecPath -Version $Version -BasePath $inputDir -OutputDirectory $packageDir -NoPackageAnalysis -NonInteractive
if ($LASTEXITCODE -ne 0) { throw "NuGet pack failed." }
$inputPackage = Join-Path $packageDir "explorer.$Version.nupkg"
if (-not (Test-Path -LiteralPath $inputPackage -PathType Leaf)) { throw "NuGet package was not generated: $inputPackage" }
Assert-NuGetPackage -PackagePath $inputPackage -ExpectedVersion $Version -BinaryPath $binaryFullPath -IconPath $iconPath -PackageIconPath $packageIconPath

$squirrelArgs = @(
    "--releasify=$inputPackage",
    "--releaseDir=$outputFullPath",
    "--setupIcon=$iconPath",
    "--no-delta",
    "--no-msi"
)
$process = Start-Process -FilePath $squirrelPath -ArgumentList $squirrelArgs -PassThru -Wait -WindowStyle Hidden
if ($process.ExitCode -ne 0) { throw "Squirrel releasify failed with exit code $($process.ExitCode)." }

$setupPath = Join-Path $outputFullPath "Setup.exe"
$releasesPath = Join-Path $outputFullPath "RELEASES"
$fullPackageName = "explorer-$Version-full.nupkg"
$fullPackagePath = Join-Path $outputFullPath $fullPackageName
foreach ($required in @($setupPath, $releasesPath, $fullPackagePath)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) { throw "Squirrel output is missing: $required" }
}
if (@(Get-ChildItem -LiteralPath $outputFullPath -Filter '*-delta.nupkg' -File).Count -ne 0) {
    throw "Delta packages are not allowed for the initial Explorer updater release."
}
Assert-NuGetPackage -PackagePath $fullPackagePath -ExpectedVersion $Version -BinaryPath $binaryFullPath -IconPath $iconPath -PackageIconPath $packageIconPath
Assert-ReleasesManifest -ReleasesPath $releasesPath -OutputDir $outputFullPath -ExpectedPackageName $fullPackageName

$templateSetup = Join-Path (Split-Path -Parent $squirrelPath) "Setup.exe"
if (-not (Test-Path -LiteralPath $templateSetup -PathType Leaf)) { throw "Squirrel template Setup.exe is missing." }
$setupHash = (Get-FileHash -LiteralPath $setupPath -Algorithm SHA256).Hash
if ($setupHash -eq (Get-FileHash -LiteralPath $templateSetup -Algorithm SHA256).Hash) {
    throw "Generated Setup.exe matches the Squirrel template; the installer payload was not embedded."
}
$setupSize = (Get-Item -LiteralPath $setupPath).Length
$templateSize = (Get-Item -LiteralPath $templateSetup).Length
$packageSize = (Get-Item -LiteralPath $fullPackagePath).Length
$minimumSize = [Math]::Max($templateSize + 65536, [Math]::Floor($packageSize * 0.20))
if ($setupSize -lt $minimumSize) {
    throw "Generated Setup.exe is unexpectedly small ($setupSize bytes; expected at least $minimumSize)."
}
Assert-NoDummyMarkers -Path $setupPath
$updateExePath = Join-Path $outputFullPath "Update.exe"
if (Test-Path -LiteralPath $updateExePath -PathType Leaf) {
    Assert-NoDummyMarkers -Path $updateExePath
} else {
    Write-Host "Update.exe was not emitted to the release root; skipping its dummy-marker scan."
}

$installerPath = Join-Path $outputFullPath "explorer-$Version-windows-amd64-installer.exe"
Copy-Item -LiteralPath $setupPath -Destination $installerPath
if ((Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash -ne $setupHash) {
    throw "Versioned installer hash does not match Setup.exe."
}

Write-Host "Squirrel.Windows $SquirrelWindowsVersion packaging complete: $outputFullPath"
