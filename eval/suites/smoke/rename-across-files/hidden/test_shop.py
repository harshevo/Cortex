import pathlib
import re
import unittest

from shop import invoice, report, tax
from shop.cart import Cart


def sample_cart():
    cart = Cart()

    cart.add("book", 12.50, 2)
    cart.add("pen", 1.99)

    return cart


class RenameTest(unittest.TestCase):

    def test_new_name_exists(self):
        self.assertTrue(callable(tax.compute_tax))

    def test_old_name_is_gone(self):
        self.assertFalse(hasattr(tax, "calc"))

    def test_no_file_still_mentions_the_old_name(self):
        package = pathlib.Path(tax.__file__).parent

        for path in package.glob("*.py"):
            source = path.read_text(encoding="utf-8")

            self.assertIsNone(
                re.search(r"\bcalc\b", source),
                f"{path.name} still uses the name calc",
            )


class RoundingTest(unittest.TestCase):

    def test_default_rounding_is_two_places(self):
        self.assertEqual(tax.compute_tax(10, 0.175), 1.75)
        self.assertEqual(tax.compute_tax(9.99), 2.0)

    def test_rounding_can_be_changed(self):
        self.assertEqual(tax.compute_tax(9.99, 0.175, rounding=3), 1.748)
        self.assertEqual(tax.compute_tax(9.99, 0.175, rounding=0), 2.0)

    def test_rounding_is_keyword_only(self):
        with self.assertRaises(TypeError):
            tax.compute_tax(10, 0.2, 3)


class CallersStillWorkTest(unittest.TestCase):

    def test_cart_total(self):
        self.assertEqual(sample_cart().total(), 32.39)

    def test_invoice(self):
        lines = invoice.render(sample_cart())

        self.assertEqual(lines, ["Subtotal: 26.99", "Tax: 5.40", "Total: 32.39"])

    def test_invoice_reduced_rate(self):
        lines = invoice.render(sample_cart(), reduced_rate=True)

        self.assertEqual(lines[1], "Tax: 1.35")

    def test_report(self):
        carts = [sample_cart(), sample_cart()]

        self.assertEqual(report.tax_collected(carts), 10.8)


if __name__ == "__main__":
    unittest.main()
