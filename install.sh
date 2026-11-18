#!/bin/bash
# nekozame-seek in ~/.local, with Textual (the TUI) next to it, and the command in ~/.local/bin.
# Run it again to update.
#   ./install.sh               Textual from PyPI, in its own Python environment
#   ./install.sh WHEELS_DIR    Textual from wheels you brought (pip download textual mutagen -d DIR):
#                              for a machine without Python's venv (Debian: python3-venv)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
DEST="$HOME/.local/share/nekozame-seek"
python3 -c 'import sys; sys.exit(sys.version_info < (3, 11))' || { echo "needs Python 3.11 or newer" >&2; exit 1; }
mkdir -p "$DEST" "$HOME/.local/bin"
if [ -n "${1:-}" ]; then
    # the wheels unpacked: they're all plain Python, so that's all installing them is
    rm -rf "$DEST/lib" && mkdir -p "$DEST/lib"
    for w in "$1"/*.whl; do python3 -m zipfile -e "$w" "$DEST/lib"; done
    run="PYTHONPATH=\"$DEST/lib\" exec python3"
elif python3 -c 'import ensurepip' 2>/dev/null; then
    python3 -m venv "$DEST/venv"
    "$DEST/venv/bin/pip" install -q --upgrade textual mutagen
    run="exec \"$DEST/venv/bin/python\""
else
    echo "no Python venv here: install python3-venv, or give this the wheels (see the top of this file)" >&2
    exit 1
fi
install -m755 "$HERE/nekozame-seek" "$DEST/nekozame-seek"
printf '#!/bin/sh\n%s "%s/nekozame-seek" "$@"\n' "$run" "$DEST" > "$HOME/.local/bin/nekozame-seek"
chmod +x "$HOME/.local/bin/nekozame-seek"
echo "installed: nekozame-seek (in ~/.local/bin)"
