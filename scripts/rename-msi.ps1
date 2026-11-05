# mise bundle(Windows) 뒤에 부른다: cargo-bundle이 만든 Hanji.msi를 자동 업데이트가 찾는 이름
# (Hanji-<버전>-x64.msi, crates/update의 asset_name)으로 바꾼다.
$ErrorActionPreference = 'Stop'
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches.Groups[1].Value
$msi = Get-ChildItem -Path target/release/bundle -Recurse -Filter Hanji.msi | Select-Object -First 1
if (-not $msi) { throw 'Hanji.msi not found under target/release/bundle' }
$out = Join-Path $msi.DirectoryName "Hanji-$version-x64.msi"
Move-Item -Force $msi.FullName $out
Write-Output $out
