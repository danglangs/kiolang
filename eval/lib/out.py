# The program's value as text: kio prints a returned `Str` as is and `Ok("..")`
# as Rust's Debug; the latter is unwrapped and unescaped. Python output passes through.
import re
import sys

lang = sys.argv[1]
raw = sys.stdin.read()
if lang == "kio":
    m = re.fullmatch(r'Ok\("(.*)"\)\n?', raw, re.S)
    if m:
        esc = {"n": "\n", "t": "\t", "r": "\r", "0": "\0", '"': '"', "\\": "\\", "'": "'"}
        raw = re.sub(r'\\(u\{([0-9a-f]+)\}|.)',
                     lambda e: chr(int(e.group(2), 16)) if e.group(2) else esc.get(e.group(1), e.group(0)),
                     m.group(1))
sys.stdout.write(raw)
