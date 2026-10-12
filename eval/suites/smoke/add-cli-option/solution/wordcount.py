"""Count the words in a text file.

Usage: python3 wordcount.py FILE [--top N]
"""

import argparse
import re
import sys
from collections import Counter


def count_words(text):
    """Return the number of whitespace-separated words in `text`."""

    return len(text.split())


def top_words(text, limit):
    """Return the `limit` most frequent words as (word, count) pairs."""

    words = re.findall(r"[a-z0-9]+", text.lower())

    counts = Counter(words)

    ranked = sorted(counts.items(), key=lambda pair: (-pair[1], pair[0]))

    return ranked[:limit]


def main(argv=None):
    parser = argparse.ArgumentParser(description="Count the words in a text file.")

    parser.add_argument("path", help="file to read")
    parser.add_argument("--top", type=int, metavar="N", help="print the N most frequent words")

    args = parser.parse_args(argv)

    with open(args.path, encoding="utf-8") as handle:
        text = handle.read()

    if args.top is None:
        print(count_words(text))

        return 0

    for word, count in top_words(text, args.top):
        print(f"{word} {count}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
