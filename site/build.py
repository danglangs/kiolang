#!/usr/bin/env python3
"""Build the kio website from the repo's markdown. Python stdlib only.

    python3 site/build.py OUT_DIR [--standalone FILE] [--no-playground]

Renders the subset of GitHub markdown the public docs use (headings with
GitHub's anchors, paragraphs, fenced code, tables, lists with indented
continuation blocks, blockquotes, raw HTML blocks, inline code, bold, links),
then checks that every relative link and #anchor in OUT_DIR resolves.
--standalone also writes the landing page as one file: CSS and images inlined,
site links pointing at the markdown on GitHub.

The playground page needs the checker built first:
    cargo build -p playground --release --target wasm32-unknown-unknown
(found under $CARGO_TARGET_DIR, default target/). Without it the build fails;
--no-playground builds the site without that page.
"""

import json

import base64
import html
import os
import re
import shutil
import sys
from html.parser import HTMLParser
from string import Template

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SITE = os.path.join(ROOT, "site")
GITHUB = "https://github.com/danglangs/kiolang"

# repo path -> (site path, title, nav label or None). Links to any other repo
# file go to GitHub.
PAGES = {
    "site/index.md": ("index.html", "kio", "Home"),
    "docs/first-job.md": ("first-job.html", "Your first job", "First job"),
    "docs/guide.md": ("guide.html", "Guide", "Guide"),
    "docs/language.md": ("language.html", "Language", "Language"),
    "docs/primer.md": ("primer.html", "kio by example", "Primer"),
    "README.md": ("readme.html", "README", "README"),
    "site/playground.html": ("playground.html", "Playground", "Playground"),
}
ASSETS = {"docs/assets/kio-unicorn.svg": "assets/kio-unicorn.svg"}
SITE_URL = "https://danglangs.github.io/kiolang/"

# The playground's examples, in picker order: site/playground/<key>.kio, a title, and the
# example whose source is the base when it opens as a `--base` diff.
EXAMPLES = [
    ("hello", "hello", None),
    ("count", "A Dir job: count words", None),
    ("sync", "Sync orders to a CRM", None),
    ("archive", "...then archive them: the diff", "sync"),
    ("tests", "Tests and a property", None),
    ("error", "An error and its hint", None),
]
WASM = os.path.join(os.environ.get("CARGO_TARGET_DIR") or os.path.join(ROOT, "target"),
                    "wasm32-unknown-unknown", "release", "playground.wasm")


class BuildError(Exception):
    pass


# ---- links

def resolve(url, base):
    """Map a link in a markdown file under repo dir `base` to its site URL."""
    if re.match(r"[a-z]+:", url) or url.startswith("#"):
        return url
    path, _, frag = url.partition("#")
    rel = os.path.normpath(os.path.join(base, path)).replace(os.sep, "/")
    frag = "#" + frag if frag else ""
    if rel in PAGES:
        return PAGES[rel][0] + frag
    if rel in ASSETS:
        return ASSETS[rel]
    full = os.path.join(ROOT, rel)
    if not os.path.exists(full) or rel.startswith(".."):
        raise BuildError(f"link to missing file: {url} (from {base or '.'}/)")
    kind = "tree" if os.path.isdir(full) else "blob"
    return f"{GITHUB}/{kind}/main/{rel}{frag}"


# ---- inline

def inline(text, base):
    slots = []

    def keep(s):
        slots.append(s)
        return f"\x00{len(slots) - 1}\x00"

    def code(m):
        s = m.group(2)
        if s.startswith(" ") and s.endswith(" ") and s.strip():
            s = s[1:-1]
        return keep(f"<code>{html.escape(s, quote=False)}</code>")

    text = re.sub(r"(`+)(.+?)(?<!`)\1(?!`)", code, text)
    text = re.sub(r"\\([!-/:-@\[-`{-~])", lambda m: keep(html.escape(m.group(1))), text)
    text = html.escape(text, quote=False)
    text = re.sub(
        r"\[([^\]]+)\]\(([^)\s]+)\)",
        lambda m: f'<a href="{html.escape(resolve(html.unescape(m.group(2)), base))}">{m.group(1)}</a>',
        text,
    )
    text = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", text)
    while "\x00" in text:
        text = re.sub(r"\x00(\d+)\x00", lambda m: slots[int(m.group(1))], text)
    return text


def slug(text, seen):
    """GitHub's heading anchor: lowercase, drop punctuation, spaces to '-'."""
    plain = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text).replace("`", "").replace("**", "")
    s = re.sub(r"[^\w\- ]", "", plain.strip().lower()).replace(" ", "-")
    n = seen.get(s, 0)
    seen[s] = n + 1
    return f"{s}-{n}" if n else s


# ---- blocks

FENCE = re.compile(r"^(\s*)(`{3,})\s*([\w-]*)")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
ITEM = re.compile(r"^(\s*)([-*]|\d+\.)\s+")


def is_table(lines, i):
    return (lines[i].lstrip().startswith("|") and i + 1 < len(lines)
            and re.match(r"^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$", lines[i + 1]))


def starts_block(lines, i):
    return bool(FENCE.match(lines[i]) or HEADING.match(lines[i]) or ITEM.match(lines[i])
                or lines[i].startswith(">") or is_table(lines, i))


def cells(row):
    row = row.strip()
    row = row[1:] if row.startswith("|") else row
    row = row[:-1] if row.endswith("|") and not row.endswith("\\|") else row
    return [c.strip().replace("\\|", "|") for c in re.split(r"(?<!\\)\|", row)]


def blocks(lines, base, seen):
    out, i, n = [], 0, len(lines)
    while i < n:
        line = lines[i]
        if not line.strip():
            i += 1
        elif m := FENCE.match(line):
            indent, ticks, lang = m.groups()
            body, i = [], i + 1
            while i < n and not re.match(rf"^\s*{ticks}\s*$", lines[i]):
                body.append(lines[i][len(indent):] if lines[i].startswith(indent) else lines[i].lstrip())
                i += 1
            i += 1
            cls = f' class="language-{lang}"' if lang else ""
            out.append(f"<pre><code{cls}>{html.escape(chr(10).join(body), quote=False)}</code></pre>")
        elif m := HEADING.match(line):
            level, text = len(m.group(1)), m.group(2)
            hid = slug(text, seen)
            out.append(f'<h{level} id="{hid}">{inline(text, base)}'
                       f'<a class="anchor" href="#{hid}" aria-label="link to this section">#</a></h{level}>')
            i += 1
        elif line.startswith("<"):
            chunk = []
            while i < n and lines[i].strip():
                chunk.append(lines[i])
                i += 1
            raw = "\n".join(chunk)
            if not raw.startswith("<!--"):
                out.append(re.sub(r'\b(src|href)="([^"]+)"',
                                  lambda m: f'{m.group(1)}="{html.escape(resolve(m.group(2), base))}"', raw))
        elif is_table(lines, i):
            head = cells(lines[i])
            rows, i = [], i + 2
            while i < n and lines[i].lstrip().startswith("|"):
                rows.append(cells(lines[i]))
                i += 1
            th = "".join(f"<th>{inline(c, base)}</th>" for c in head)
            trs = "".join("<tr>" + "".join(f"<td>{inline(c, base)}</td>" for c in r) + "</tr>" for r in rows)
            out.append(f'<div class="table"><table><thead><tr>{th}</tr></thead><tbody>{trs}</tbody></table></div>')
        elif line.startswith(">"):
            quote = []
            while i < n and lines[i].startswith(">"):
                quote.append(re.sub(r"^> ?", "", lines[i]))
                i += 1
            out.append(f"<blockquote>{blocks(quote, base, seen)}</blockquote>")
        elif m := ITEM.match(line):
            ordered = m.group(2)[0].isdigit()
            items, loose = [], False
            while i < n and (m := ITEM.match(lines[i])) and m.group(2)[0].isdigit() == ordered:
                width = len(m.group(0))
                item, i = [lines[i][width:]], i + 1
                while i < n:
                    if lines[i].strip() and not lines[i].startswith(" "):
                        break
                    if not lines[i].strip():
                        nxt = next((l for l in lines[i:] if l.strip()), "")
                        if not nxt.startswith(" "):
                            break
                        loose = True
                    item.append(lines[i][width:] if lines[i].startswith(" " * width) else lines[i].lstrip())
                    i += 1
                items.append(item)
                if i < n and not lines[i].strip():
                    nxt = next((j for j in range(i, n) if lines[j].strip()), n)
                    if nxt < n and ITEM.match(lines[nxt]):
                        loose, i = True, nxt
            tag = "ol" if ordered else "ul"
            lis = []
            for item in items:
                body = blocks(item, base, seen)
                if not loose and body.startswith("<p>") and body.count("<p>") == 1:
                    body = body[3:].replace("</p>", "", 1)
                lis.append(f"<li>{body}</li>")
            out.append(f"<{tag}>{''.join(lis)}</{tag}>")
        else:
            para = []
            while i < n and lines[i].strip() and not (para and starts_block(lines, i)):
                para.append(lines[i].strip())
                i += 1
            out.append(f"<p>{inline(' '.join(para), base)}</p>")
    return "\n".join(out)


# ---- pages

def render(src):
    if src == "site/playground.html":
        return playground()
    with open(os.path.join(ROOT, src), encoding="utf-8") as f:
        lines = f.read().split("\n")
    base = "" if src == "site/index.md" else os.path.dirname(src)
    return blocks(lines, base, {})


def playground():
    """site/playground.html with the examples in its JSON block."""
    examples = {}
    for key, title, base in EXAMPLES:
        with open(os.path.join(SITE, "playground", key + ".kio"), encoding="utf-8") as f:
            examples[key] = {"title": title, "src": f.read(), "base": base}
    with open(os.path.join(SITE, "playground.html"), encoding="utf-8") as f:
        body = f.read()
    return body.replace("<!-- examples -->", json.dumps(examples).replace("</", "<\\/"))


def page(src, body):
    out, title, _ = PAGES[src]
    nav = "".join(
        f'<a href="{o}"{" aria-current=page" if o == out else ""}>{label}</a>'
        for o, _, label in PAGES.values())
    with open(os.path.join(SITE, "template.html"), encoding="utf-8") as f:
        tpl = Template(f.read())
    return tpl.substitute(
        title=title if out == "index.html" else f"{title} · kio",
        cls="landing" if out == "index.html" else "doc",
        nav=nav, body=body,
        source=f"{GITHUB}/blob/main/{src}")


class Links(HTMLParser):
    def __init__(self):
        super().__init__()
        self.ids, self.refs = set(), []

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if "id" in a:
            self.ids.add(a["id"])
        for k in ("href", "src"):
            if a.get(k):
                self.refs.append(a[k])


def check_links(out):
    parsed = {}
    for name in [p[0] for p in PAGES.values()]:
        p = Links()
        with open(os.path.join(out, name), encoding="utf-8") as f:
            p.feed(f.read())
        parsed[name] = p
    bad = []
    for name, p in parsed.items():
        for ref in p.refs:
            if re.match(r"[a-z]+:", ref):
                continue
            path, _, frag = ref.partition("#")
            target = path or name
            if not os.path.exists(os.path.join(out, target)):
                bad.append(f"{name}: {ref} (no file)")
            elif frag and (target not in parsed or frag not in parsed[target].ids):
                bad.append(f"{name}: {ref} (no #{frag})")
    return bad


def standalone(out, dest):
    with open(os.path.join(out, "index.html"), encoding="utf-8") as f:
        doc = f.read()
    with open(os.path.join(SITE, "style.css"), encoding="utf-8") as f:
        doc = doc.replace('<link rel="stylesheet" href="style.css">', f"<style>\n{f.read()}</style>")
    for src, rel in ASSETS.items():
        with open(os.path.join(ROOT, src), "rb") as f:
            data = base64.b64encode(f.read()).decode()
        doc = doc.replace(f'"{rel}"', f'"data:image/svg+xml;base64,{data}"')
    for src, (o, _, _) in PAGES.items():
        url = SITE_URL + o if src.endswith(".html") else f"{GITHUB}/blob/main/{src}"
        doc = re.sub(rf'href="{re.escape(o)}(#[^"]*)?"',
                     lambda m: f'href="{url}{m.group(1) or ""}"', doc)
    os.makedirs(os.path.dirname(os.path.abspath(dest)), exist_ok=True)
    with open(dest, "w", encoding="utf-8") as f:
        f.write(doc)


def main(argv):
    with_playground = "--no-playground" not in argv
    argv = [a for a in argv if a != "--no-playground"]
    if len(argv) not in (2, 4) or (len(argv) == 4 and argv[2] != "--standalone"):
        sys.exit(__doc__)
    out = argv[1]
    if not with_playground:
        del PAGES["site/playground.html"]
    elif not os.path.exists(WASM):
        sys.exit(f"build: no {WASM}\n  build it first: cargo build -p playground --release "
                 "--target wasm32-unknown-unknown\n  or pass --no-playground to build without that page")
    os.makedirs(out, exist_ok=True)
    try:
        for src in PAGES:
            with open(os.path.join(out, PAGES[src][0]), "w", encoding="utf-8") as f:
                f.write(page(src, render(src)))
    except BuildError as e:
        sys.exit(f"build: {e}")
    shutil.copy(os.path.join(SITE, "style.css"), out)
    if with_playground:
        shutil.copy(WASM, os.path.join(out, "playground.wasm"))
    for src, rel in ASSETS.items():
        os.makedirs(os.path.join(out, os.path.dirname(rel)), exist_ok=True)
        shutil.copy(os.path.join(ROOT, src), os.path.join(out, rel))
    bad = check_links(out)
    if bad:
        sys.exit("broken links:\n  " + "\n  ".join(bad))
    if len(argv) == 4:
        standalone(out, argv[3])
    print(f"built {len(PAGES)} pages in {out}; all relative links resolve")


if __name__ == "__main__":
    main(sys.argv)
