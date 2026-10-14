# Builds Cortex in release mode and packs it with everything it needs, so
# that the result runs on another Windows machine with nothing else
# installed. The same as scripts/package.sh does on macOS and Linux.
#
#     scripts\package.ps1             target\package\Cortex
#     scripts\package.ps1 -Zip        also packs it into dist\ to give to someone
#     scripts\package.ps1 -Fresh      builds the Python side again from nothing
#
# The machine that builds needs `cargo` and `uv`, and the embedding model:
# it is taken from the Hugging Face cache, or downloaded when it is not
# there (it is a gated model, so that needs `hf auth login` first). The
# machine that runs the result needs none of these.
param(
    [switch]$Zip,
    [switch]$Fresh
)

$ErrorActionPreference = "Stop"

Set-Location (Join-Path $PSScriptRoot "..")

# Stops the script when a program it ran has failed.
function Check($what) {
    if ($LASTEXITCODE -ne 0) { throw "$what failed" }
}

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches.Groups[1].Value

$brain = if ($env:CORTEX_BRAIN_DIR) { $env:CORTEX_BRAIN_DIR } else { "..\brain" }
$pythonVersion = "3.12"
$model = "google/embeddinggemma-2"
$modelFolder = "embeddinggemma-2"

# Where cargo puts what it builds, when that is not `target`.
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }

$stage = "$target\package"
$services = "$stage\services"
$python = "$services\python\python.exe"

if (-not (Get-Command uv -ErrorAction SilentlyContinue)) { throw "uv is needed to build the Python side: https://docs.astral.sh/uv/" }
if (-not (Test-Path "$brain\cortex\brain\server.py")) { throw "the brain project was not found at $brain; set CORTEX_BRAIN_DIR" }

# ---------------------------------------------------------------- programs

cargo build --release -p cortex-gui -p cortex-cli
Check "cargo build"

# ------------------------------------------------------------- Python side

# Installing the libraries takes minutes, so it is done again only when
# the list of them has changed.
$stamp = "$pythonVersion $((Get-FileHash ml\uv.lock).Hash)"
$had = if (Test-Path "$services\.libraries") { Get-Content "$services\.libraries" -Raw } else { "" }

if ($Fresh -or $had.Trim() -ne $stamp) {
    if (Test-Path $services) { Remove-Item -Recurse -Force $services }
    New-Item -ItemType Directory -Force $services | Out-Null

    # A standalone interpreter: it finds its own files relative to itself,
    # so the folder can be moved anywhere.
    uv python install $pythonVersion --install-dir "$stage\interpreters" --no-bin
    Check "uv python install"

    $interpreter = Get-ChildItem "$stage\interpreters" -Directory -Filter "cpython-$pythonVersion.*" | Select-Object -First 1
    Move-Item $interpreter.FullName "$services\python"
    Remove-Item -Recurse -Force "$stage\interpreters"

    # The libraries the lock file names, without those only training uses.
    uv export --project ml --frozen --no-dev --no-emit-project --no-hashes `
        --prune pandas --prune pyarrow `
        --output-file "$stage\requirements.txt" | Out-Null
    Check "uv export"

    uv pip install --python $python --break-system-packages --no-deps -r "$stage\requirements.txt"
    Check "uv pip install"

    # Parts of Python no service uses.
    foreach ($unused in "Lib\test", "Lib\idlelib", "Lib\tkinter", "Lib\turtledemo", "Lib\ensurepip",
                        "Lib\site-packages\torch\include", "tcl", "include") {
        if (Test-Path "$services\python\$unused") { Remove-Item -Recurse -Force "$services\python\$unused" }
    }

    Set-Content "$services\.libraries" $stamp
}

# The services' own code is small and changes often: copied every time.
foreach ($folder in "$services\ml", "$services\brain") {
    if (Test-Path $folder) { Remove-Item -Recurse -Force $folder }
}

New-Item -ItemType Directory -Force "$services\ml", "$services\brain\cortex" | Out-Null

Copy-Item -Recurse ml\cortex_ml "$services\ml\cortex_ml"
Copy-Item ml\hope.pt "$services\ml\hope.pt"

Copy-Item "$brain\cortex\__init__.py", "$brain\cortex\profile.py" "$services\brain\cortex\"
Copy-Item -Recurse "$brain\cortex\brain" "$services\brain\cortex\brain"

Get-ChildItem "$services\ml", "$services\brain" -Recurse -Directory -Filter __pycache__ | Remove-Item -Recurse -Force

# The embedding model's weights.
if (-not (Test-Path "$services\models\$modelFolder\model.safetensors")) {
    if (Test-Path "$services\models\$modelFolder") { Remove-Item -Recurse -Force "$services\models\$modelFolder" }
    New-Item -ItemType Directory -Force "$services\models" | Out-Null

    $cache = if ($env:HF_HUB_CACHE) { $env:HF_HUB_CACHE }
             elseif ($env:HF_HOME) { "$env:HF_HOME\hub" }
             else { "$env:USERPROFILE\.cache\huggingface\hub" }

    $snapshots = "$cache\models--$($model -replace '/', '--')\snapshots"
    $snapshot = if (Test-Path $snapshots) { Get-ChildItem $snapshots -Directory | Select-Object -First 1 } else { $null }

    if ($snapshot -and (Test-Path "$($snapshot.FullName)\model.safetensors")) {
        # The cache may keep links to the files; robocopy copies the files.
        robocopy $snapshot.FullName "$services\models\$modelFolder" /E /NFL /NDL /NJH /NJS | Out-Null
        if ($LASTEXITCODE -ge 8) { throw "copying the embedding model failed" }
    } else {
        Write-Host "downloading $model ..."

        $target = "$services\models\$modelFolder" -replace '\\', '/'

        & $python -c "from huggingface_hub import snapshot_download; snapshot_download('$model', local_dir='$target')"
        Check "downloading the embedding model"

        if (Test-Path "$services\models\$modelFolder\.cache") { Remove-Item -Recurse -Force "$services\models\$modelFolder\.cache" }
    }
}

# The package is never written to while it runs, so Python cannot keep
# its compiled files as it goes. They are made here instead, and marked
# as good whatever the dates on the files become when they are unpacked.
& $python -m compileall -q -j 0 --invalidation-mode unchecked-hash `
    "$services\python\Lib" "$services\ml" "$services\brain" | Out-Null

# --------------------------------------------------------------- the package

$package = "$stage\Cortex"

if (Test-Path $package) { Remove-Item -Recurse -Force $package }
New-Item -ItemType Directory -Force $package | Out-Null

Copy-Item "$target\release\cortex-gui.exe", "$target\release\cortex.exe" $package
Copy-Item -Recurse $services "$package\services"

Write-Host "built $package"

if ($Zip) {
    New-Item -ItemType Directory -Force dist | Out-Null

    $archive = "dist\Cortex-$version-windows-x64.zip"

    if (Test-Path $archive) { Remove-Item $archive }

    # The `tar` that comes with Windows writes zip files, and far faster
    # than Compress-Archive does for a folder this size.
    tar -a -c -f $archive -C $stage Cortex
    Check "packing"

    Write-Host "packed $archive"
}
