#!/usr/bin/env python3
"""Case-preserving buzz -> kuzz rename over the tracked tree.

buzz -> kuzz, Buzz -> Kuzz, BUZZ -> KUZZ in file contents and paths.
Strings that are stored on the server, sent to already-installed clients,
or point at third-party hosts are protected (see KEEP_* below) and left as-is.
A line containing the marker `kuzz-rename: keep` is never changed, so legacy
compatibility code written after the rename survives a re-run.

Usage: python scripts/kuzz_rename.py [--dry-run] [--report PATH]
Run from the repository root on a clean tree. Re-running is a no-op.
"""
import argparse
import os
import re
import subprocess
import sys
from collections import Counter

SELF = "scripts/kuzz_rename.py"
KEEP_MARKER = "kuzz-rename: keep"

# Files whose contents are never rewritten (paths may still move).
SKIP_CONTENT = [
    re.compile(r"(^|/)migrations/[^/]+\.sql$"),
    re.compile(r"(^|/)desktop/src-tauri/src/testdata/hpke_nsec_backup_v1\.json$"),
    re.compile(r"(^|/)Podfile\.lock$"),
    re.compile(r"^schema/schema\.sql$"),
    re.compile(r"app_attest_transcripts\.json$"),
]

# Per-file regexes whose matches are kept verbatim.
KEEP_IN_FILE = {
    # Redis key prefix shared with the running relay.
    "crates/buzz-pubsub/src/topic.rs": [r'(?<=PREFIX: &str = )"buzz"'],
}

# Lines in lockfiles carrying hashes are left alone.
LOCK_FILE = re.compile(r"(\.lock|lock\.yaml)$")
LOCK_HASH_LINE = re.compile(r"(integrity|checksum|sha256|sha512|hash)\b", re.I)

# Third-party or externally registered names (regex, matched case-sensitively).
KEEP_EXTERNAL = [
    r"github\.com/(?:block|salman1993|squareup|attacker)/buzz[\w.-]*",
    r"api\.github\.com/repos/block/buzz[\w.-]*",
    r"jijeegee/buzz\b",
    r"ghcr\.io/block/buzz[\w.-]*",
    r"[\w.-]*\.?buzz\.xyz",
    r"buzz\.block(?:\.builderlab)?\.xyz",
    r"buzz-oss\.stage\.blox\.sqprod\.co",
    r"buzz\.mesh\.bb-production\.com",
    r"relay\.buzz\.place",
    r"jihobuzz",
    r"play\.google\.com/store/apps/details\?id=xyz\.block\.buzz[\w.]*",
    r"buzz-pi-acp",
]

# Wire / stored / crypto tokens (exact strings). Filled from the protocol audit.
KEEP_TOKENS: list[str] = []
KEEP_TOKENS_FILE = os.path.join(os.path.dirname(__file__), "kuzz_rename_keep.txt")

# Reverse-DNS app identity: xyz.block.buzz(.x) -> app.kuzz(.x)
IDENTITY_SUBS = [
    (re.compile(r"xyz\.block\.buzz"), "app.kuzz"),
    (re.compile(r"xyz/block/buzz"), "app/kuzz"),
    (re.compile(r"xyz_block_buzz"), "app_kuzz"),
]

WORD = re.compile(r"buzz|Buzz|BUZZ")
CASE = {"buzz": "kuzz", "Buzz": "Kuzz", "BUZZ": "KUZZ"}


def load_keep_tokens():
    toks = list(KEEP_TOKENS)
    if os.path.exists(KEEP_TOKENS_FILE):
        with open(KEEP_TOKENS_FILE, encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if line and not line.startswith("#"):
                    toks.append(line)
    return toks


def build_protector(tokens):
    parts = list(KEEP_EXTERNAL)
    # Longest first so that buzz-channel-x wins over buzz-channel.
    for tok in sorted(set(tokens), key=len, reverse=True):
        parts.append(r"(?<![\w-])" + re.escape(tok) + r"(?![\w-])")
    return re.compile("|".join(parts))


def rename_text(text, protector, stats, extra=None):
    out_lines = []
    for line in text.splitlines(keepends=True):
        if KEEP_MARKER in line or not WORD.search(line):
            out_lines.append(line)
            continue
        kept = []

        def stash(m):
            kept.append(m.group(0))
            stats["kept:" + m.group(0)[:60]] += 1
            return "\x00%d\x00" % (len(kept) - 1)

        work = protector.sub(stash, line)
        if extra is not None:
            work = extra.sub(stash, work)
        for rx, rep in IDENTITY_SUBS:
            work = rx.sub(rep, work)
        work, n = WORD.subn(lambda m: CASE[m.group(0)], work)
        stats["replaced"] += n
        work = re.sub(r"\x00(\d+)\x00", lambda m: kept[int(m.group(1))], work)
        out_lines.append(work)
    return "".join(out_lines)


def rename_path(path):
    new = path
    for rx, rep in IDENTITY_SUBS:
        new = rx.sub(rep, new)
    return WORD.sub(lambda m: CASE[m.group(0)], new)


def is_binary(data):
    return b"\x00" in data[:8192]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--report")
    args = ap.parse_args()

    files = subprocess.check_output(["git", "ls-files", "-z"]).decode("utf-8").split("\0")
    files = [f for f in files if f and os.path.isfile(f)]
    protector = build_protector(load_keep_tokens())
    stats = Counter()
    changed = 0
    for f in files:
        if f in (SELF, "scripts/kuzz_rename_keep.txt"):
            continue
        if any(rx.search(f) for rx in SKIP_CONTENT):
            continue
        with open(f, "rb") as fh:
            data = fh.read()
        if is_binary(data):
            continue
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            stats["non-utf8:" + f] += 1
            continue
        if LOCK_FILE.search(f):
            new = "".join(
                line if LOCK_HASH_LINE.search(line) else rename_text(line, protector, stats)
                for line in text.splitlines(keepends=True)
            )
        else:
            extra = KEEP_IN_FILE.get(f)
            extra = re.compile("|".join(extra)) if extra else None
            new = rename_text(text, protector, stats, extra)
        if new != text:
            changed += 1
            if not args.dry_run:
                with open(f, "wb") as fh:
                    fh.write(new.encode("utf-8"))

    moves = [(f, rename_path(f)) for f in files if rename_path(f) != f and f != SELF]
    if not args.dry_run:
        for old, new in moves:
            os.makedirs(os.path.dirname(new) or ".", exist_ok=True)
            subprocess.check_call(["git", "mv", "-k", old, new])
    print("files changed: %d, paths moved: %d, replacements: %d"
          % (changed, len(moves), stats["replaced"]))
    if args.report:
        with open(args.report, "w", encoding="utf-8", newline="\n") as fh:
            for k, v in sorted(stats.items(), key=lambda kv: -kv[1]):
                fh.write("%d\t%s\n" % (v, k))
            for old, new in moves:
                fh.write("move\t%s\t%s\n" % (old, new))
    return 0


if __name__ == "__main__":
    sys.exit(main())
