"""Invoices."""

from shop import tax


def render(cart, reduced_rate=False):
    """Return the invoice for `cart` as lines of text."""

    rate = 0.05 if reduced_rate else tax.STANDARD_RATE

    subtotal = cart.subtotal()
    tax_due = tax.calc(subtotal, rate)

    return [
        f"Subtotal: {subtotal:.2f}",
        f"Tax: {tax_due:.2f}",
        f"Total: {subtotal + tax_due:.2f}",
    ]
