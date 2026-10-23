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

# Set by the Enter key handler just before it lets a line run; read and
# cleared by `prompt` once that line has finished, to log it to history.
$global:ExecGuardPendingCommand = $null
$global:ExecGuardPendingOutcome = $null
$global:ExecGuardPendingStarted = $null

function global:ExecGuardWillRun([string]$command, [string]$outcome) {
    $global:ExecGuardPendingCommand = $command
    $global:ExecGuardPendingOutcome = $outcome
    $global:ExecGuardPendingStarted = Get-Date
}

# `prompt` runs after every command, the same role Bash's PROMPT_COMMAND
# plays -- it's where we finally know the real exit code and duration.
# Whatever `prompt` was already defined (a custom profile, oh-my-posh, ...)
# is kept and called afterwards, so we don't clobber it.
$global:ExecGuardPreviousPrompt = if (Test-Path Function:\prompt) {
    Get-Content Function:\prompt
} else {
    { "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) " }
}

function global:prompt {
    # Captured first, before anything else here can overwrite them.
    $execguardExitCode = $LASTEXITCODE
    $execguardSucceeded = $?

    if ($global:ExecGuardPendingCommand) {
        $resolvedExitCode = if ($null -ne $execguardExitCode -and $execguardExitCode -ne 0) {
            $execguardExitCode
        } elseif ($execguardSucceeded) {
            0
        } else {
            1
        }
        $durationMs = [int]((Get-Date) - $global:ExecGuardPendingStarted).TotalMilliseconds

        try {
            $env:EXECGUARD_COMMAND = $global:ExecGuardPendingCommand
            & $global:ExecGuardExe record --shell powershell --cwd (Get-Location).Path `
                --exit-code $resolvedExitCode --duration-ms $durationMs `
                --outcome $global:ExecGuardPendingOutcome --command-from-env 2>&1 | Out-Null
        } catch {
            # Logging history must never break the prompt.
        } finally {
            Remove-Item Env:EXECGUARD_COMMAND -ErrorAction SilentlyContinue
        }

        $global:ExecGuardPendingCommand = $null
    }

    & $global:ExecGuardPreviousPrompt
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

    # Erase the typed line from the screen and print the messages in its place.
    # InvokePrompt then draws the prompt and command once, below the messages.
    $showMessages = {
        param([string[]] $lines)
        $readLine::SetCursorPosition(0)
        $top = [Console]::CursorTop
        $width = [Console]::BufferWidth
        $rows = [int][Math]::Max(1, [Math]::Ceiling(([Console]::CursorLeft + $line.Length) / $width))
        [Console]::SetCursorPosition(0, $top)
        [Console]::Write(' ' * ($width * $rows - 1))
        [Console]::SetCursorPosition(0, $top)
        foreach ($text in $lines) { [Console]::WriteLine($text) }
    }

    switch ($code) {
        0 {
            ExecGuardWillRun $line 'ALLOW'
            $readLine::AcceptLine()
        }
        10 {
            & $showMessages $messages
            $readLine::InvokePrompt()
            ExecGuardWillRun $line 'WARN'
            $readLine::AcceptLine()
        }
        20 {
            & $showMessages $messages
            [Console]::Write('Run it anyway? [y/N] ')
            $key = [Console]::ReadKey($true)
            [Console]::WriteLine($key.KeyChar)
            $readLine::InvokePrompt()
            if ("$($key.KeyChar)" -eq 'y') {
                ExecGuardWillRun $line 'CONFIRM'
                $readLine::AcceptLine()
            } else {
                $readLine::SetCursorPosition($cursor)
            }
        }
        30 {
            & $showMessages ($messages + "ExecGuard blocked this command because it is on your reject list. Edit it, or set `$env:EXECGUARD_DISABLE = '1' to bypass.")
            $readLine::InvokePrompt()
            $readLine::SetCursorPosition($cursor)
        }
        default {
            & $showMessages ($messages + "ExecGuard failed (exit code $code); running the command UNCHECKED.")
            $readLine::InvokePrompt()
            ExecGuardWillRun $line 'UNCHECKED'
            $readLine::AcceptLine()
        }
    }
}

Write-Host "ExecGuard active ($global:ExecGuardExe)" -ForegroundColor DarkGray