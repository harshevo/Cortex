"""Stock keeping for a small warehouse."""


class Inventory:

    def __init__(self):
        self._items = {}

    def add(self, name, quantity, unit_price):
        """Add stock. Adding an item that already exists increases its quantity."""

        if quantity <= 0:
            raise ValueError("quantity must be positive")

        if name in self._items:
            self._items[name]["quantity"] += quantity
        else:
            self._items[name] = {"quantity": quantity, "unit_price": unit_price}

    def remove(self, name, quantity):
        """Take stock out. An item whose quantity reaches zero is dropped."""

        item = self._items[name]

        if quantity > item["quantity"]:
            raise ValueError(f"only {item['quantity']} of {name} in stock")

        item["quantity"] -= quantity

        if item["quantity"] == 0:
            del self._items[name]

    def quantity(self, name):
        """Return how many of `name` are in stock (0 if unknown)."""

        item = self._items.get(name)

        return item["quantity"] if item else 0

    def names(self):
        """Return the names of the items in stock, sorted."""

        return sorted(self._items)

    def total_value(self):
        """Return the value of all stock: quantity times unit price, summed."""

        return sum(item["quantity"] * item["unit_price"] for item in self._items.values())
