import os
import re
from collections import Counter

text = open(os.path.join(os.environ["INPUT"], "text.txt")).read()
counts = Counter(w.lower() for w in re.findall(r"[A-Za-z]+", text))
for word, n in sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))[:5]:
    print(word, n)
