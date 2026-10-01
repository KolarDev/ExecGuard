# ExecGuard integration for PowerShell (requires PSReadLine).
# Load it from your profile:   . "C:\path\to\execguard.ps1"
# Bypass for one session:      $env:EXECGUARD_DISABLE = '1'

if (-not (Get-Module -Name PSReadLine)) {
    Write-Warning "ExecGuard: PSReadLine is not loaded; commands will NOT be checked."
    return
}

# Find execguard.exe once: first on PATH, then in this repo's release build.
$global:ExecGuardExe = (Get-Command execguard -CommandType Application -ErrorAction SilentlyContinue |
    Select-Object -First 1).Path
if (-not $global:ExecGuardExe) {
    $candidate = Join-Path $PSScriptRoot '..\target\release\execguard.exe'
    if (Test-Path $candidate) { $global:ExecGuardExe = (Resolve-Path $candidate).Path }
}
if (-not $global:ExecGuardExe) {
    Write-Warning "ExecGuard: execguard.exe not found. Run 'cargo build --release' or add it to PATH. Commands will NOT be checked."
    return
}

# Elevation can't change during a session, so check it once at load time.
$global:ExecGuardElevated = if ($IsWindows -or $PSVersionTable.PSEdition -eq 'Desktop') {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    ([Security.Principal.WindowsPrincipal]$identity).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
} else {
    (id -u) -eq '0'
}

Set-PSReadLineKeyHandler -Key Enter -BriefDescription 'ExecGuardAcceptLine' `
    -Description 'Check the command with ExecGuard before running it' -ScriptBlock {

    $readLine = [Microsoft.PowerShell.PSConsoleReadLine]

    $line = $null
    $cursor = $null
    $readLine::GetBufferState([ref]$line, [ref]$cursor)

    # Nothing to check, or ExecGuard switched off for this session.
    if ([string]::IsNullOrWhiteSpace($line) -or $env:EXECGUARD_DISABLE -eq '1') {
        $readLine::AcceptLine()
        return
    }

    $checkArgs = @('check', '--shell', 'powershell')
    if ($global:ExecGuardElevated) { $checkArgs += '--elevated' }
    $checkArgs += '--command-from-env'

    # Don't let ExecGuard overwrite the user's own $LASTEXITCODE.
    $savedExitCode = $global:LASTEXITCODE
    $env:EXECGUARD_COMMAND = $line
    try {
        $messages = @(& $global:ExecGuardExe @checkArgs 2>&1 | ForEach-Object { "$_" })
        $code = $LASTEXITCODE
    } catch {
        $messages = @("execguard: could not run ExecGuard ($($_.Exception.Message))")
        $code = -1
    } finally {
        Remove-Item Env:EXECGUARD_COMMAND -ErrorAction SilentlyContinue
        $global:LASTEXITCODE = $savedExitCode
    }

    if ($messages.Count -gt 0) {
        [Console]::WriteLine()
        foreach ($message in $messages) { [Console]::WriteLine($message) }
    }

    switch ($code) {
        0 {
            $readLine::AcceptLine()
        }
        10 {
            $readLine::InvokePrompt()
            $readLine::AcceptLine()
        }
        20 {
            [Console]::Write('Run it anyway? [y/N] ')
            $key = [Console]::ReadKey($true)
            [Console]::WriteLine($key.KeyChar)
            $readLine::InvokePrompt()
            if ("$($key.KeyChar)" -eq 'y') { $readLine::AcceptLine() }
        }
        30 {
            [Console]::WriteLine("ExecGuard blocked this command. Edit it, or set `$env:EXECGUARD_DISABLE = '1' to bypass.")
            $readLine::InvokePrompt()
        }
        default {
            [Console]::WriteLine("ExecGuard failed (exit code $code); running the command UNCHECKED.")
            $readLine::InvokePrompt()
            $readLine::AcceptLine()
        }
    }
}

Write-Host "ExecGuard active ($global:ExecGuardExe)" -ForegroundColor DarkGray