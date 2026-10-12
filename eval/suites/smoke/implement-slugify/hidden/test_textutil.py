import unittest

from textutil import slugify


class SlugifyTest(unittest.TestCase):

    def test_punctuation_and_case(self):
        self.assertEqual(slugify("Hello, World!"), "hello-world")

    def test_accents_are_reduced(self):
        self.assertEqual(slugify("Crème Brûlée"), "creme-brulee")

    def test_runs_collapse_to_one_hyphen(self):
        self.assertEqual(slugify("  --a   b--  "), "a-b")

    def test_digits_are_kept(self):
        self.assertEqual(slugify("Version 2.0"), "version-2-0")

    def test_max_length_cuts_the_slug(self):
        self.assertEqual(slugify("hello world again", max_length=11), "hello-world")

    def test_max_length_does_not_leave_a_trailing_hyphen(self):
        self.assertEqual(slugify("hello world again", max_length=6), "hello")

    def test_empty_result_raises(self):
        with self.assertRaises(ValueError):
            slugify("!!!")

        with self.assertRaises(ValueError):
            slugify("")


if __name__ == "__main__":
    unittest.main()
