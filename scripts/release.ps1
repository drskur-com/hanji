# mise release / mise publish (Windows).
# release: Hanji-<버전>-x64.msi를 GitHub 릴리스 v<버전>(초안)에 올린다. 릴리스가 없으면 지금 커밋으로 만든다.
# publish(-Publish): 초안을 공개하고 최신으로 표시한다.
param([switch]$Publish)
$ErrorActionPreference = 'Stop'
$repo = 'drskur-com/writer'
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches.Groups[1].Value
$tag = "v$version"

if ($Publish) {
    gh release edit $tag -R $repo --draft=false --latest
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    gh release view $tag -R $repo --json url -q .url
    exit 0
}

$msi = Get-ChildItem -Path target/release/bundle -Recurse -Filter "Hanji-$version-x64.msi" | Select-Object -First 1
if (-not $msi) { throw "Hanji-$version-x64.msi not found. Run mise bundle first." }
if (git status --porcelain --untracked-files=no) { throw 'There are uncommitted changes.' }
$commit = git rev-parse HEAD
git fetch -q origin
if (-not (git branch -r --contains $commit)) { throw "Commit $commit is not on origin. Run git push first." }

gh release view $tag -R $repo *> $null
if ($LASTEXITCODE -eq 0) {
    gh release upload $tag $msi.FullName -R $repo --clobber
} else {
    gh release create $tag $msi.FullName -R $repo --draft --target $commit --title "Hanji $version" --generate-notes
}
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
gh release view $tag -R $repo --json url,isDraft,assets -q '.url, "draft: \(.isDraft)", (.assets[].name)'
