#!/usr/bin/env bash
# Regenerate NotoSansCJKjp-UI.otf — a glyph subset of Noto Sans CJK JP
# covering every non-Latin literal in the GUI sources plus kana and
# CJK punctuation blocks. Run this whenever a Japanese string is added
# or the subset is missing glyphs. Requires fonttools (pyftsubset) and
# a system Noto Sans CJK (fonts-noto-cjk on Debian/Ubuntu).
set -euo pipefail
cd "$(dirname "$0")/.."

SRC_FONT="${SRC_FONT:-/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc}"
OUT="assets/NotoSansCJKjp-UI.otf"

python3 - <<'PY'
import re, glob
chars = set()
for f in glob.glob('src/*.rs'):
    for m in re.findall(r'"([^"]*)"', open(f, encoding='utf8').read()):
        chars.update(c for c in m if ord(c) > 0x2000)
for lo, hi in [(0x3000, 0x30FF), (0xFF00, 0xFFEF)]:
    chars.update(chr(c) for c in range(lo, hi + 1))
open('/tmp/ja-glyphs.txt', 'w', encoding='utf8').write(''.join(sorted(chars)))
print(f"{len(chars)} glyphs to subset")
PY

pyftsubset "$SRC_FONT" --font-number=0 --text-file=/tmp/ja-glyphs.txt \
  --unicodes="U+3040-30FF,U+3000-303F,U+FF00-FFEF,U+2010-205E,U+00B7" \
  --output-file="$OUT" --layout-features='*' --no-hinting --desubroutinize
ls -la "$OUT"
