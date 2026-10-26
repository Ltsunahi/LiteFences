Describe 'Test-UndoVisibleAcceptanceReport' {
    $checkerPath = Join-Path $PSScriptRoot '..\Test-UndoVisibleAcceptanceReport.ps1'
    $projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
    $artifactsRoot = Join-Path $projectRoot 'artifacts'
    $script:fixtureSequence = 0

    function New-UndoVisibleFixture {
        $script:fixtureSequence++
        $ticks = [DateTime]::UtcNow.Ticks + $script:fixtureSequence
        $directory = Join-Path $artifactsRoot ("undo-group-visible-{0}-{1}" -f $PID, $ticks)
        [void](New-Item -ItemType Directory -Path $directory)
        $sourceBytes = [Text.Encoding]::ASCII.GetBytes("LiteFences undo visible acceptance fixture.`r`nThis file must remain unchanged.`r`n")
        [IO.File]::WriteAllBytes((Join-Path $directory 'source.txt'), $sourceBytes)
        $config = [ordered]@{ version = 1; nested = [ordered]@{ alpha = 'a'; beta = @(1, 2, 3) }; entries = @('first', 'second') }
        $seed = [ordered]@{ version = 1; nested = [ordered]@{ alpha = 'a'; beta = @(1, 2, 3) }; entries = @('first', 'second') }
        $report = [ordered]@{
            status = 'passed'; undo_kind = 'group'; scope = 'synthetic acceptance fixture'
            data_dir = [System.IO.Path]::GetFullPath($directory); process_id = [int]$PID
            undo_fixture_seeded = $true; undo_fixture_restored = $true
            source_unchanged = $true; disk_restored_exact = $true; timeout_fired = $false
        }
        $jsonOptions = @{ Depth = 12 }
        $report | ConvertTo-Json @jsonOptions | Set-Content -LiteralPath (Join-Path $directory 'undo-visible-report.json') -Encoding UTF8
        $config | ConvertTo-Json @jsonOptions | Set-Content -LiteralPath (Join-Path $directory 'config.json') -Encoding UTF8
        $seed | ConvertTo-Json @jsonOptions | Set-Content -LiteralPath (Join-Path $directory 'seed-config.json') -Encoding UTF8
        return [pscustomobject]@{ Directory = $directory; ReportPath = (Join-Path $directory 'undo-visible-report.json'); ConfigPath = (Join-Path $directory 'config.json'); SeedPath = (Join-Path $directory 'seed-config.json') }
    }

    function Set-FixtureJson($Path, $Value) {
        $Value | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $Path -Encoding UTF8
    }

    function Invoke-Checker($DataDir, $ExpectedStatus = 'passed') {
        $processInfo = New-Object System.Diagnostics.ProcessStartInfo
        $processInfo.FileName = (Get-Process -Id $PID).Path
        $quote = { param($Value) '"' + ([string]$Value).Replace('"', '\"') + '"' }
        $processInfo.Arguments = '-NoProfile -File ' + (& $quote $checkerPath) + ' -DataDir ' + (& $quote $DataDir) + ' -ExpectedStatus ' + (& $quote $ExpectedStatus)
        $processInfo.UseShellExecute = $false
        $processInfo.CreateNoWindow = $true
        $processInfo.RedirectStandardOutput = $true
        $processInfo.RedirectStandardError = $true
        $process = New-Object System.Diagnostics.Process
        $process.StartInfo = $processInfo
        try {
            [void]$process.Start()
            $stdoutTask = $process.StandardOutput.ReadToEndAsync()
            $stderrTask = $process.StandardError.ReadToEndAsync()
            if (-not $process.WaitForExit(10000)) {
                # Only stop the exact checker child created by this test.
                $process.Kill()
                $process.WaitForExit()
                throw 'Report checker timed out after 10 seconds.'
            }
            $stdout = $stdoutTask.Result
            $stderr = $stderrTask.Result
            $parsed = $null
            if (-not [string]::IsNullOrWhiteSpace($stdout)) {
                try { $parsed = $stdout | ConvertFrom-Json } catch { }
            }
            return [pscustomobject]@{ ExitCode = $process.ExitCode; Result = $parsed; Output = $stdout; Error = $stderr }
        } finally {
            $process.Dispose()
        }
    }

    It 'accepts object property reordering, including nested objects' {
        $fixture = New-UndoVisibleFixture
        $config = [ordered]@{ entries = @('first', 'second'); nested = [ordered]@{ beta = @(1, 2, 3); alpha = 'a' }; version = 1 }
        Set-FixtureJson $fixture.ConfigPath $config
        Set-FixtureJson $fixture.SeedPath ([ordered]@{ nested = [ordered]@{ alpha = 'a'; beta = @(1, 2, 3) }; version = 1; entries = @('first', 'second') })
        $result = Invoke-Checker $fixture.Directory
        $result.ExitCode | Should Be 0
        $result.Result.verification_passed | Should Be $true
    }

    It 'rejects array order changes even when disk_restored_exact is true' {
        $fixture = New-UndoVisibleFixture
        Set-FixtureJson $fixture.ConfigPath ([ordered]@{ version = 1; nested = [ordered]@{ alpha = 'a'; beta = @(1, 2, 3) }; entries = @('second', 'first') })
        $result = Invoke-Checker $fixture.Directory
        $result.ExitCode | Should Be 1
        $result.Result.verification_passed | Should Be $false
    }

    It 'rejects a string true flag' {
        $fixture = New-UndoVisibleFixture
        $report = Get-Content -LiteralPath $fixture.ReportPath -Raw | ConvertFrom-Json
        $report.undo_fixture_seeded = 'true'
        Set-FixtureJson $fixture.ReportPath $report
        $result = Invoke-Checker $fixture.Directory
        $result.ExitCode | Should Be 1
        $result.Error | Should Match 'must be a JSON boolean'
    }

    It 'rejects a numeric string process_id' {
        $fixture = New-UndoVisibleFixture
        $report = Get-Content -LiteralPath $fixture.ReportPath -Raw | ConvertFrom-Json
        $report.process_id = [string]$PID
        Set-FixtureJson $fixture.ReportPath $report
        $result = Invoke-Checker $fixture.Directory
        $result.ExitCode | Should Be 1
        $result.Result.verification_passed | Should Be $false
    }

    It 'rejects expected failed status when all four flags are true' {
        $fixture = New-UndoVisibleFixture
        $report = Get-Content -LiteralPath $fixture.ReportPath -Raw | ConvertFrom-Json
        $report.status = 'failed'
        Set-FixtureJson $fixture.ReportPath $report
        $result = Invoke-Checker $fixture.Directory 'failed'
        $result.ExitCode | Should Be 1
        $result.Result.verification_passed | Should Be $false
    }

    It 'rejects project root and dot-segment DataDir paths' {
        $fixture = New-UndoVisibleFixture
        $rootResult = Invoke-Checker $projectRoot
        $rootResult.ExitCode | Should Not Be 0
        $dotPath = Join-Path (Split-Path -Parent $fixture.Directory) ('.\' + (Split-Path -Leaf $fixture.Directory))
        $dotResult = Invoke-Checker $dotPath
        $dotResult.ExitCode | Should Not Be 0
    }
}
