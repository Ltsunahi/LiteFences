Describe 'Summarize-RenderBench' {
    $scriptPath = Join-Path $PSScriptRoot '..\Summarize-RenderBench.ps1'

    function Invoke-Summary($Runs) {
        $fixture = New-TemporaryFile
        try {
            @{ schema_version = 1; runs = $Runs } | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $fixture.FullName
            (& pwsh -NoProfile -File $scriptPath -ResultsPath $fixture.FullName) | ConvertFrom-Json
        } finally {
            Remove-Item -LiteralPath $fixture.FullName -Force
        }
    }

    It 'calculates odd and even medians and ignores null or nonnumeric metrics' {
        $runs = @(
            @{ scenario='empty'; round=1; status='completed'; sample_count=2; metrics=@{ private_bytes_p95=10; working_set_p95=1; private_working_set_p95=$null; cpu_normalized_percent=3; gdi_objects=1; user_objects=1 } },
            @{ scenario='empty'; round=2; status='completed'; sample_count=2; metrics=@{ private_bytes_p95=20; working_set_p95=2; private_working_set_p95=4; cpu_normalized_percent=5; gdi_objects=2; user_objects='bad' } },
            @{ scenario='empty'; round=3; status='completed'; sample_count=2; metrics=@{ private_bytes_p95=30; working_set_p95=3; private_working_set_p95=8; cpu_normalized_percent=7; gdi_objects=3; user_objects=3 } },
            @{ scenario='empty'; round=4; status='completed'; sample_count=2; metrics=@{ private_bytes_p95=40; working_set_p95=4; private_working_set_p95=12; cpu_normalized_percent=9; gdi_objects=4; user_objects=4 } }
        )
        $result = Invoke-Summary $runs
        $result.scenarios.empty.runs | Should Be 4
        $result.scenarios.empty.metrics.private_bytes_p95 | Should Be 25
        $result.scenarios.empty.metrics.private_working_set_p95 | Should Be 8
        $result.scenarios.empty.metrics.user_objects | Should Be 3
    }

    It 'pairs only matching rounds and supports negative differences' {
        $runs = @(
            @{ scenario='empty'; round=1; status='completed'; sample_count=2; metrics=@{ private_bytes_p95=100 } },
            @{ scenario='empty'; round=2; status='completed'; sample_count=2; metrics=@{ private_bytes_p95=200 } },
            @{ scenario='gdi-surface'; round=1; status='completed'; sample_count=2; metrics=@{ private_bytes_p95=90 } },
            @{ scenario='gdi-surface'; round=3; status='completed'; sample_count=2; metrics=@{ private_bytes_p95=500 } }
        )
        $result = Invoke-Summary $runs
        $result.increments.'gdi-surface-empty'.valid_pair_count | Should Be 1
        $difference = $result.increments.'gdi-surface-empty'.difference_median
        # Windows PowerShell + legacy Pester can stringify -10.0 differently
        # from the integer literal. Require a numeric value, then compare numerically.
        ($difference -is [ValueType] -and $difference -isnot [bool]) | Should Be $true
        ($difference -eq -10) | Should Be $true
    }

    It 'rejects duplicate scenario and round' {
        $runs = @(
            @{ scenario='empty'; round=1; status='completed'; sample_count=2; metrics=@{} },
            @{ scenario='empty'; round=1; status='completed'; sample_count=2; metrics=@{} }
        )
        $fixture = New-TemporaryFile
        try {
            @{ schema_version = 1; runs = $runs } | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $fixture.FullName
            $message = ''
            try { & $scriptPath -ResultsPath $fixture.FullName | Out-Null } catch { $message = $_.Exception.Message }
            $message | Should Match 'duplicate scenario\+round'
        } finally {
            Remove-Item -LiteralPath $fixture.FullName -Force
        }
    }

    It 'rejects missing files, invalid schema, non-array runs, and invalid integer fields' {
        $message = ''
        try { & $scriptPath -ResultsPath (Join-Path $TestDrive 'missing.json') | Out-Null } catch { $message = $_.Exception.Message }
        $message | Should Match 'does not exist'

        $cases = @(
            @{ json = @{ schema_version = 2; runs = @() } | ConvertTo-Json -Depth 10; message = '*schema_version*' },
            @{ json = '{"schema_version":1,"runs":"not-an-array"}'; message = '*JSON array*' },
            @{ json = @{ schema_version = 1; runs = @(@{ scenario='empty'; round=0; status='completed'; sample_count=2; metrics=@{} }) } | ConvertTo-Json -Depth 10; message = '*invalid round*' },
            @{ json = @{ schema_version = 1; runs = @(@{ scenario='empty'; round=1; status='completed'; sample_count=-1; metrics=@{} }) } | ConvertTo-Json -Depth 10; message = '*invalid sample_count*' }
        )
        foreach ($case in $cases) {
            $fixture = New-TemporaryFile
            try {
                Set-Content -LiteralPath $fixture.FullName -Value $case.json
                $message = ''
                try { & $scriptPath -ResultsPath $fixture.FullName | Out-Null } catch { $message = $_.Exception.Message }
                $message | Should Match ($case.message.Trim('*'))
            } finally {
                Remove-Item -LiteralPath $fixture.FullName -Force
            }
        }
    }

    It 'does not coerce numeric strings into metric values' {
        $runs = @(@{ scenario='empty'; round=1; status='completed'; sample_count=2; metrics=@{ private_bytes_p95='123'; working_set_p95=10 } })
        $result = Invoke-Summary $runs
        $result.scenarios.empty.metrics.private_bytes_p95 | Should BeNullOrEmpty
        $result.scenarios.empty.metrics.working_set_p95 | Should Be 10
    }
}
