"""Shopping cart."""

from shop.tax import compute_tax


class Cart:

    def __init__(self):
        self.lines = []

    def add(self, name, price, quantity=1):
        self.lines.append((name, price, quantity))

    def subtotal(self):
        return sum(price * quantity for _, price, quantity in self.lines)

    def total(self):
        subtotal = self.subtotal()

        return round(subtotal + compute_tax(subtotal), 2)
