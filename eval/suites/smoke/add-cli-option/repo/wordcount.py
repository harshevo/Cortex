"""Count the words in a text file.

Usage: python3 wordcount.py FILE
"""

import argparse
import sys


def count_words(text):
    """Return the number of whitespace-separated words in `text`."""

    return len(text.split())


def main(argv=None):
    parser = argparse.ArgumentParser(description="Count the words in a text file.")

    parser.add_argument("path", help="file to read")

    args = parser.parse_args(argv)

    with open(args.path, encoding="utf-8") as handle:
        text = handle.read()

    print(count_words(text))

    return 0


if __name__ == "__main__":
    sys.exit(main())
