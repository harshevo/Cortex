import unittest

from pagination import page_count, paginate


class PaginateTest(unittest.TestCase):

    def setUp(self):
        self.items = list(range(1, 11))

    def test_first_page_starts_at_the_first_item(self):
        self.assertEqual(paginate(self.items, 1, 3), [1, 2, 3])

    def test_middle_page(self):
        self.assertEqual(paginate(self.items, 2, 3), [4, 5, 6])

    def test_last_partial_page(self):
        self.assertEqual(paginate(self.items, 4, 3), [10])

    def test_page_past_the_end_is_empty(self):
        self.assertEqual(paginate(self.items, 5, 3), [])

    def test_invalid_arguments(self):
        with self.assertRaises(ValueError):
            paginate(self.items, 0, 3)

        with self.assertRaises(ValueError):
            paginate(self.items, 1, 0)


class PageCountTest(unittest.TestCase):

    def test_exact_fit(self):
        self.assertEqual(page_count(list(range(9)), 3), 3)

    def test_partial_last_page_is_counted(self):
        self.assertEqual(page_count(list(range(10)), 3), 4)

    def test_empty_list_has_no_pages(self):
        self.assertEqual(page_count([], 3), 0)

    def test_invalid_page_size(self):
        with self.assertRaises(ValueError):
            page_count([1], 0)


if __name__ == "__main__":
    unittest.main()
