[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Source,

    [Parameter(Mandatory = $true)]
    [string]$Destination,

    [switch]$HashSource = $true,
    [switch]$VerifyDestination,
    [switch]$DryRun,
    [string]$TicketNumber,
    [switch]$SkipHistory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-DataRoot {
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        return Join-Path -Path $HOME -ChildPath "AppData\Local\Graft"
    }

    return Join-Path -Path $env:LOCALAPPDATA -ChildPath "Graft"
}

function Ensure-Directory {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        New-Item -Path $Path -ItemType Directory -Force | Out-Null
    }
}

function Resolve-TransferSource {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        throw "Source path does not exist: $Path"
    }

    $item = Get-Item -LiteralPath $Path
    if ($item.PSIsContainer) {
        return [pscustomobject]@{
            SourceRoot = $item.FullName
            FileFilter = $null
            SourceMode = "Folder"
            SourceLeaf = $null
        }
    }

    $parent = Split-Path -Path $item.FullName -Parent
    $leaf = Split-Path -Path $item.FullName -Leaf

    return [pscustomobject]@{
        SourceRoot = $parent
        FileFilter = $leaf
        SourceMode = "File"
        SourceLeaf = $leaf
    }
}

function Validate-Paths {
    param(
        [Parameter(Mandatory = $true)][string]$SourceRoot,
        [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][string]$SourceMode
    )

    if ([string]::IsNullOrWhiteSpace($SourceRoot)) {
        throw "Source path cannot be empty."
    }

    if ([string]::IsNullOrWhiteSpace($Destination)) {
        throw "Destination path cannot be empty."
    }

    if (-not (Test-Path -LiteralPath $SourceRoot)) {
        throw "Source path does not exist: $SourceRoot"
    }

    if ((Test-Path -LiteralPath $Destination) -and -not (Get-Item -LiteralPath $Destination).PSIsContainer) {
        throw "Destination must be a folder."
    }

    $sourceFull = (Resolve-Path -LiteralPath $SourceRoot).Path
    $destFull = $null

    if (Test-Path -LiteralPath $Destination) {
        $destFull = (Resolve-Path -LiteralPath $Destination).Path
    }
    else {
        $destFull = [System.IO.Path]::GetFullPath($Destination)
    }

    if ($sourceFull.TrimEnd('\\') -ieq $destFull.TrimEnd('\\')) {
        throw "Source and destination cannot be the same path."
    }

    if ($SourceMode -eq "Folder") {
        if ($destFull.StartsWith($sourceFull.TrimEnd('\\') + '\\', [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Destination cannot be inside source directory."
        }
    }

    $invalid = @('<', '>', '"', '|', '?', '*')
    foreach ($ch in $invalid) {
        if ($SourceRoot.Contains($ch) -or $Destination.Contains($ch)) {
            throw "Path contains invalid character: $ch"
        }
    }
}

function Get-LargeFilesWanArgs {
    param(
        [Parameter(Mandatory = $true)][string]$SourceRoot,
        [Parameter(Mandatory = $true)][string]$Destination,
        [string]$FileFilter,
        [switch]$DryRun
    )

    $args = @(
        $SourceRoot,
        $Destination
    )

    if (-not [string]::IsNullOrWhiteSpace($FileFilter)) {
        $args += $FileFilter
    }

    # Large Files over WAN preset from the Rust implementation.
    $args += @(
        "/E",
        "/COPY:DAT",
        "/DCOPY:DAT",
        "/J",
        "/NP",
        "/R:3",
        "/W:5",
        "/MT:8"
    )

    if ($DryRun) {
        $args += "/L"
    }

    return $args
}

function Quote-Arg {
    param([Parameter(Mandatory = $true)][string]$Text)

    if ($Text -match "\s") {
        return '"' + $Text.Replace('"', '\"') + '"'
    }

    return $Text
}

function Get-ExitMessage {
    param([Parameter(Mandatory = $true)][int]$Code)

    switch ($Code) {
        0 { return "No files copied. Source and destination are in sync." }
        1 { return "Files copied successfully." }
        2 { return "Extra files or directories detected in destination." }
        3 { return "Files copied and extra files detected." }
        4 { return "Mismatched files or directories detected." }
        { $_ -ge 5 -and $_ -le 7 } { return "Files copied with some issues." }
        { $_ -ge 8 -and $_ -le 15 } { return "Some files could not be copied (errors occurred)." }
        default { return "Serious error or unknown robocopy status." }
    }
}

function Get-FileHashes {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Mode,
        [string]$SingleFileName
    )

    $results = @{}
    $failures = New-Object System.Collections.Generic.List[string]

    if ($Mode -eq "File") {
        $target = Join-Path -Path $Root -ChildPath $SingleFileName
        try {
            $hash = Get-FileHash -LiteralPath $target -Algorithm SHA256
            $results[$SingleFileName] = [pscustomobject]@{
                RelativePath = $SingleFileName
                Hash = $hash.Hash.ToLowerInvariant()
                Size = (Get-Item -LiteralPath $target).Length
            }
        }
        catch {
            $failures.Add($SingleFileName)
        }

        return [pscustomobject]@{ Map = $results; Failures = $failures }
    }

    $files = Get-ChildItem -LiteralPath $Root -File -Recurse -Force
    $baseLength = $Root.TrimEnd('\\').Length + 1

    foreach ($file in $files) {
        $relative = $file.FullName.Substring($baseLength)
        try {
            $hash = Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256
            $results[$relative] = [pscustomobject]@{
                RelativePath = $relative
                Hash = $hash.Hash.ToLowerInvariant()
                Size = $file.Length
            }
        }
        catch {
            $failures.Add($relative)
        }
    }

    return [pscustomobject]@{ Map = $results; Failures = $failures }
}

function Compare-HashMaps {
    param(
        [Parameter(Mandatory = $true)]$SourceMap,
        [Parameter(Mandatory = $true)]$DestinationMap
    )

    $matched = 0
    $mismatched = New-Object System.Collections.Generic.List[string]
    $missing = New-Object System.Collections.Generic.List[string]
    $extra = New-Object System.Collections.Generic.List[string]

    foreach ($key in $SourceMap.Keys) {
        if (-not $DestinationMap.ContainsKey($key)) {
            $missing.Add($key)
            continue
        }

        if ($SourceMap[$key].Hash -ceq $DestinationMap[$key].Hash) {
            $matched++
        }
        else {
            $mismatched.Add($key)
        }
    }

    foreach ($key in $DestinationMap.Keys) {
        if (-not $SourceMap.ContainsKey($key)) {
            $extra.Add($key)
        }
    }

    return [pscustomobject]@{
        MatchedCount = $matched
        Mismatched = $mismatched
        MissingInDestination = $missing
        ExtraInDestination = $extra
    }
}

function Save-HistoryEntry {
    param(
        [Parameter(Mandatory = $true)]$Entry,
        [Parameter(Mandatory = $true)][string]$HistoryPath
    )

    $history = [pscustomobject]@{
        entries = @()
        max_entries = 100
    }

    if (Test-Path -LiteralPath $HistoryPath) {
        try {
            $existing = Get-Content -LiteralPath $HistoryPath -Raw | ConvertFrom-Json
            if ($null -ne $existing) {
                $history = $existing
                if (-not ($history.PSObject.Properties.Name -contains "entries")) {
                    $history | Add-Member -NotePropertyName entries -NotePropertyValue @() -Force
                }

                if (-not ($history.PSObject.Properties.Name -contains "max_entries")) {
                    if ($history.PSObject.Properties.Name -contains "maxEntries") {
                        $history | Add-Member -NotePropertyName max_entries -NotePropertyValue ([int]$history.maxEntries) -Force
                    }
                    else {
                        $history | Add-Member -NotePropertyName max_entries -NotePropertyValue 100 -Force
                    }
                }
            }
        }
        catch {
            # If history is corrupt, overwrite with a new one.
        }
    }

    $newEntries = @($Entry) + @($history.entries)
    $maxEntries = [int]$history.max_entries
    if ($newEntries.Count -gt $maxEntries) {
        $newEntries = $newEntries[0..($maxEntries - 1)]
    }

    $history.entries = $newEntries

    $history | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $HistoryPath -Encoding UTF8
}

$startedAt = Get-Date
$dataRoot = Get-DataRoot
$logRoot = Join-Path -Path $dataRoot -ChildPath "logs"
$historyPath = Join-Path -Path $dataRoot -ChildPath "history.json"
Ensure-Directory -Path $dataRoot
Ensure-Directory -Path $logRoot

$resolved = Resolve-TransferSource -Path $Source
Validate-Paths -SourceRoot $resolved.SourceRoot -Destination $Destination -SourceMode $resolved.SourceMode

$args = Get-LargeFilesWanArgs -SourceRoot $resolved.SourceRoot -Destination $Destination -FileFilter $resolved.FileFilter -DryRun:$DryRun
$displayArgs = $args | ForEach-Object { Quote-Arg -Text $_ }
$commandText = "robocopy " + ($displayArgs -join " ")

$runStamp = Get-Date -Format "yyyy-MM-dd_HH-mm-ss"
$ticketSegment = ""
if (-not [string]::IsNullOrWhiteSpace($TicketNumber)) {
    $ticketSafe = ($TicketNumber -replace "[^A-Za-z0-9_-]", "_")
    $ticketSegment = "_" + $ticketSafe
}
$logPath = Join-Path -Path $logRoot -ChildPath ("graft_{0}{1}.log" -f $runStamp, $ticketSegment)

$logLines = New-Object System.Collections.Generic.List[string]
$logLines.Add(("[{0}] Preset: Large Files over WAN" -f (Get-Date -Format "yyyy-MM-dd HH:mm:ss"))) | Out-Null
$logLines.Add(("[{0}] Command: {1}" -f (Get-Date -Format "yyyy-MM-dd HH:mm:ss"), $commandText)) | Out-Null

Write-Host "Preset: Large Files over WAN"
Write-Host "Command: $commandText"
Write-Host ""

$robocopyOutput = & robocopy @args 2>&1
$exitCode = $LASTEXITCODE

foreach ($line in $robocopyOutput) {
    $text = $line.ToString()
    Write-Host $text
    $logLines.Add($text) | Out-Null
}

$exitMessage = Get-ExitMessage -Code $exitCode
Write-Host ""
Write-Host "Robocopy exit code: $exitCode"
Write-Host $exitMessage
$logLines.Add(("Robocopy exit code: {0}" -f $exitCode)) | Out-Null
$logLines.Add($exitMessage) | Out-Null

$hashVerificationFailed = $false
if ($exitCode -lt 8 -and ($HashSource -or $VerifyDestination)) {
    Write-Host ""
    Write-Host "Starting hashing..."

    $sourceHashes = $null
    $destinationHashes = $null

    if ($HashSource) {
        Write-Host "Hashing source..."
        $sourceHashes = Get-FileHashes -Root $resolved.SourceRoot -Mode $resolved.SourceMode -SingleFileName $resolved.SourceLeaf
        Write-Host ("Source hashed files: {0}" -f $sourceHashes.Map.Count)
        if ($sourceHashes.Failures.Count -gt 0) {
            Write-Warning ("Source hash failures: {0}" -f $sourceHashes.Failures.Count)
            $hashVerificationFailed = $true
        }
    }

    if ($VerifyDestination) {
        Write-Host "Hashing destination..."
        $destinationRoot = $Destination
        $destinationMode = "Folder"
        $destinationLeaf = $null

        if ($resolved.SourceMode -eq "File") {
            $destinationMode = "File"
            $destinationLeaf = $resolved.SourceLeaf
        }

        $destinationHashes = Get-FileHashes -Root $destinationRoot -Mode $destinationMode -SingleFileName $destinationLeaf
        Write-Host ("Destination hashed files: {0}" -f $destinationHashes.Map.Count)
        if ($destinationHashes.Failures.Count -gt 0) {
            Write-Warning ("Destination hash failures: {0}" -f $destinationHashes.Failures.Count)
            $hashVerificationFailed = $true
        }
    }

    if ($HashSource -and $VerifyDestination) {
        $verification = Compare-HashMaps -SourceMap $sourceHashes.Map -DestinationMap $destinationHashes.Map

        Write-Host ""
        Write-Host "Hash Verification Report"
        Write-Host ("Matched: {0}" -f $verification.MatchedCount)
        Write-Host ("Mismatched: {0}" -f $verification.Mismatched.Count)
        Write-Host ("Missing in destination: {0}" -f $verification.MissingInDestination.Count)
        Write-Host ("Extra in destination: {0}" -f $verification.ExtraInDestination.Count)

        $logLines.Add("Hash Verification Report") | Out-Null
        $logLines.Add(("Matched: {0}" -f $verification.MatchedCount)) | Out-Null
        $logLines.Add(("Mismatched: {0}" -f $verification.Mismatched.Count)) | Out-Null
        $logLines.Add(("Missing in destination: {0}" -f $verification.MissingInDestination.Count)) | Out-Null
        $logLines.Add(("Extra in destination: {0}" -f $verification.ExtraInDestination.Count)) | Out-Null

        if ($verification.Mismatched.Count -gt 0 -or $verification.MissingInDestination.Count -gt 0) {
            $hashVerificationFailed = $true
        }
    }
}
elseif ($exitCode -ge 8 -and ($HashSource -or $VerifyDestination)) {
    Write-Warning "Skipping hash operations because robocopy reported copy errors."
}

$finishedAt = Get-Date
$duration = [math]::Round(($finishedAt - $startedAt).TotalSeconds, 2)

$logLines.Add(("DurationSeconds: {0}" -f $duration)) | Out-Null
$logLines | Set-Content -LiteralPath $logPath -Encoding UTF8
Write-Host ""
Write-Host "Saved log: $logPath"

if (-not $SkipHistory) {
    $historyEntry = [pscustomobject]@{
        timestamp = (Get-Date).ToString("o")
        source = $Source
        destination = $Destination
        sourceMode = $resolved.SourceMode
        command = $commandText
        preset = "Large Files over WAN"
        dryRun = [bool]$DryRun
        hashSource = [bool]$HashSource
        verifyDestination = [bool]$VerifyDestination
        ticketNumber = $TicketNumber
        exitCode = [int]$exitCode
        exitMessage = $exitMessage
        hashVerificationFailed = [bool]$hashVerificationFailed
        durationSeconds = $duration
        logPath = $logPath
    }

    Save-HistoryEntry -Entry $historyEntry -HistoryPath $historyPath
}

# Preserve robocopy semantics by returning robocopy's exit code.
exit $exitCode
