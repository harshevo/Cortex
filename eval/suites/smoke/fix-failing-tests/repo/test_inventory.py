import unittest

from inventory import Inventory


class InventoryTest(unittest.TestCase):

    def setUp(self):
        self.inventory = Inventory()

        self.inventory.add("bolt", 100, 0.25)
        self.inventory.add("gear", 4, 12.0)

    def test_adding_an_existing_item_increases_its_quantity(self):
        self.inventory.add("bolt", 50, 0.25)

        self.assertEqual(self.inventory.quantity("bolt"), 150)

    def test_adding_a_non_positive_quantity_is_rejected(self):
        with self.assertRaises(ValueError):
            self.inventory.add("bolt", 0, 0.25)

    def test_remove_reduces_stock(self):
        self.inventory.remove("bolt", 30)

        self.assertEqual(self.inventory.quantity("bolt"), 70)

    def test_removing_more_than_is_in_stock_is_rejected(self):
        with self.assertRaises(ValueError):
            self.inventory.remove("gear", 5)

        self.assertEqual(self.inventory.quantity("gear"), 4)

    def test_removing_everything_drops_the_item(self):
        self.inventory.remove("gear", 4)

        self.assertEqual(self.inventory.names(), ["bolt"])

    def test_removing_an_unknown_item_raises_key_error(self):
        with self.assertRaises(KeyError):
            self.inventory.remove("widget", 1)

    def test_total_value_multiplies_quantity_by_price(self):
        self.assertEqual(self.inventory.total_value(), 73.0)


if __name__ == "__main__":
    unittest.main()
