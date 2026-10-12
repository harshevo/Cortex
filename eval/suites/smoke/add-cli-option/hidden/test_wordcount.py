import os
import subprocess
import sys
import tempfile
import unittest

TEXT = "The cat. The dog! the bird, a cat?\n"


def run_wordcount(*arguments):
    """Run wordcount.py on a file containing TEXT and return its output lines."""

    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False, encoding="utf-8") as handle:
        handle.write(TEXT)

    try:
        completed = subprocess.run(
            [sys.executable, "wordcount.py", handle.name, *arguments],
            capture_output=True,
            text=True,
            timeout=30,
        )
    finally:
        os.unlink(handle.name)

    if completed.returncode != 0:
        raise AssertionError(f"wordcount.py failed: {completed.stderr}")

    return completed.stdout.splitlines()


class WordcountTest(unittest.TestCase):

    def test_without_the_option_prints_the_total(self):
        self.assertEqual(run_wordcount(), ["8"])

    def test_top_words_are_ranked_by_count(self):
        self.assertEqual(run_wordcount("--top", "2"), ["the 3", "cat 2"])

    def test_ties_are_alphabetical(self):
        self.assertEqual(run_wordcount("--top", "4"), ["the 3", "cat 2", "a 1", "bird 1"])

    def test_asking_for_more_than_exist_prints_all(self):
        lines = run_wordcount("--top", "50")

        self.assertEqual(lines, ["the 3", "cat 2", "a 1", "bird 1", "dog 1"])


if __name__ == "__main__":
    unittest.main()
