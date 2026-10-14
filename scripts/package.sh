#!/bin/sh
# Builds Cortex in release mode and packs it with everything it needs, so
# that the result runs on another machine with nothing else installed.
# For macOS and Linux; Windows has scripts/package.ps1.
#
#     scripts/package.sh              macOS: target/package/Cortex.app
#                                     Linux: target/package/cortex
#     scripts/package.sh --zip        also packs it into dist/ to give to someone
#     scripts/package.sh --install    macOS: also copies it to /Applications
#     scripts/package.sh --fresh      builds the Python side again from nothing
#
# What goes in beside the two programs is a `services` folder:
#
#     python/    an interpreter of its own, with torch and the rest installed
#     ml/        cortex_ml and the trained HOPE weights
#     brain/     the mind, from the `brain` project beside this checkout
#     models/    the embedding model's weights
#
# The machine that builds needs `cargo` and `uv`, and the embedding model:
# it is taken from the Hugging Face cache, or downloaded when it is not
# there (it is a gated model, so that needs `hf auth login` first). The
# machine that runs the result needs none of these.
#
# A package runs on the system and processor it was built on. Build on
# each kind of machine you want to give it to.
set -eu

cd "$(dirname "$0")/.."

zip=no
install=no
fresh=no

for argument in "$@"; do
    case "$argument" in
        --zip) zip=yes ;;
        --install) install=yes ;;
        --fresh) fresh=yes ;;
        *) echo "unknown argument: $argument" >&2; exit 2 ;;
    esac
done

system=$(uname -s)
machine=$(uname -m)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

brain=${CORTEX_BRAIN_DIR:-../brain}
python_version=3.12
model=google/embeddinggemma-2
model_folder=embeddinggemma-2

# Where cargo puts what it builds, when that is not `target`.
target=${CARGO_TARGET_DIR:-target}

stage=$target/package
services=$stage/services
python=$services/python/bin/python3

command -v uv >/dev/null || { echo "uv is needed to build the Python side: https://docs.astral.sh/uv/" >&2; exit 1; }
[ -f "$brain/cortex/brain/server.py" ] || { echo "the brain project was not found at $brain; set CORTEX_BRAIN_DIR" >&2; exit 1; }

# ---------------------------------------------------------------- programs

cargo build --release -p cortex-gui -p cortex-cli

# ------------------------------------------------------------- Python side

# Installing the libraries takes minutes, so it is done again only when
# the list of them has changed.
stamp="$python_version $(cksum < ml/uv.lock)"

if [ "$fresh" = yes ] || [ "$(cat "$services/.libraries" 2>/dev/null)" != "$stamp" ]; then
    rm -rf "$services"
    mkdir -p "$services"

    # A standalone interpreter: it finds its own files relative to itself,
    # so the folder can be moved anywhere.
    uv python install "$python_version" --install-dir "$stage/interpreters" --no-bin
    # Beside the folder there is a link to it named without the last number.
    mv "$stage"/interpreters/cpython-"$python_version".* "$services/python"
    rm -rf "$stage/interpreters"

    # The libraries the lock file names, without those only training uses.
    extra=
    [ "$system" = Darwin ] && extra="--extra apple"

    # shellcheck disable=SC2086
    uv export --project ml --frozen --no-dev --no-emit-project --no-hashes $extra \
        --prune pandas --prune pyarrow \
        --output-file "$stage/requirements.txt" >/dev/null

    add() {
        uv pip install --python "$python" --break-system-packages --no-deps "$@"
    }

    if [ "$system" = Linux ]; then
        # The usual Linux build of torch brings several gigabytes of
        # NVIDIA libraries. The package carries the one for processors.
        grep -E '^(torch|torchvision)==' "$stage/requirements.txt" | sed 's/[ ;].*//' > "$stage/torch.txt"
        grep -vE '^(torch==|torchvision==|nvidia-|triton|cuda-)' "$stage/requirements.txt" > "$stage/rest.txt"

        add --index-url https://download.pytorch.org/whl/cpu -r "$stage/torch.txt"
        add -r "$stage/rest.txt"
    else
        add -r "$stage/requirements.txt"
    fi

    # Parts of Python no service uses.
    library=$services/python/lib/python$python_version

    rm -rf "$library/test" "$library/idlelib" "$library/tkinter" "$library/turtledemo" \
        "$library/ensurepip" "$library/site-packages/torch/include" \
        "$services"/python/lib/tcl* "$services"/python/lib/tk* "$services"/python/lib/itcl* \
        "$services"/python/lib/thread* "$services/python/share" "$services/python/include"

    echo "$stamp" > "$services/.libraries"
fi

# The services' own code is small and changes often: copied every time.
rm -rf "$services/ml" "$services/brain"
mkdir -p "$services/ml" "$services/brain/cortex"

cp -R ml/cortex_ml "$services/ml/cortex_ml"
cp ml/hope.pt "$services/ml/hope.pt"

cp "$brain/cortex/__init__.py" "$brain/cortex/profile.py" "$services/brain/cortex/"
cp -R "$brain/cortex/brain" "$services/brain/cortex/brain"

find "$services/ml" "$services/brain" -name __pycache__ -type d -prune -exec rm -rf {} +

# The embedding model's weights.
if [ ! -f "$services/models/$model_folder/model.safetensors" ]; then
    rm -rf "$services/models/$model_folder"
    mkdir -p "$services/models"

    cache=${HF_HUB_CACHE:-${HF_HOME:-$HOME/.cache/huggingface}/hub}
    snapshot=$(ls -d "$cache/models--$(echo "$model" | sed 's|/|--|')"/snapshots/*/ 2>/dev/null | head -1)

    if [ -n "$snapshot" ] && [ -e "$snapshot/model.safetensors" ]; then
        # The cache keeps links to the files; the package gets the files.
        cp -RL "$snapshot" "$services/models/$model_folder"
    else
        echo "downloading $model ..."

        "$python" -c "
from huggingface_hub import snapshot_download
snapshot_download('$model', local_dir='$services/models/$model_folder')
"
        rm -rf "$services/models/$model_folder/.cache"
    fi
fi

# The package is never written to while it runs, so Python cannot keep
# its compiled files as it goes. They are made here instead, and marked
# as good whatever the dates on the files become when they are unpacked.
"$python" -m compileall -q -j 0 --invalidation-mode unchecked-hash \
    "$services/python/lib" "$services/ml" "$services/brain" >/dev/null 2>&1 || true

# --------------------------------------------------------------- the package

if [ "$system" = Darwin ]; then
    app=$stage/Cortex.app
    package=$app

    rm -rf "$app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

    cp "$target/release/cortex-gui" "$target/release/cortex" "$app/Contents/MacOS/"

    # A clone: no second copy of two gigabytes is kept on the disk.
    cp -Rc "$services" "$app/Contents/Resources/services"

    cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Cortex</string>
    <key>CFBundleDisplayName</key>
    <string>Cortex</string>
    <key>CFBundleIdentifier</key>
    <string>dev.cortex.app</string>
    <key>CFBundleExecutable</key>
    <string>cortex-gui</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

    # Signed without an identity. That is enough for the Mac it was built
    # on. Another Mac holds a downloaded copy back until the person lets
    # it through; see the note printed at the end.
    codesign --force --sign - "$app"

    archive=dist/Cortex-$version-macos-$machine.zip
else
    package=$stage/cortex

    rm -rf "$package"
    mkdir -p "$package"

    cp "$target/release/cortex-gui" "$target/release/cortex" "$package/"
    cp -al "$services" "$package/services" 2>/dev/null || cp -a "$services" "$package/services"

    archive=dist/cortex-$version-linux-$machine.tar.gz
fi

echo "built $package ($(du -sh "$package" | cut -f1))"

if [ "$zip" = yes ]; then
    mkdir -p dist
    rm -f "$archive"

    if [ "$system" = Darwin ]; then
        # ditto keeps the signature and the bundle's layout intact.
        ditto -c -k --keepParent "$package" "$archive"
    else
        tar -czf "$archive" -C "$stage" cortex
    fi

    echo "packed $archive ($(du -sh "$archive" | cut -f1))"
fi

if [ "$install" = yes ] && [ "$system" = Darwin ]; then
    rm -rf /Applications/Cortex.app
    cp -Rc "$package" /Applications/Cortex.app
    echo "installed /Applications/Cortex.app"
fi

if [ "$system" = Darwin ]; then
    cat <<NOTE

On another Mac, after unpacking, macOS will refuse to open a downloaded
copy because it is not signed by a registered developer. Once, in Terminal:

    xattr -cr /Applications/Cortex.app

NOTE
fi
