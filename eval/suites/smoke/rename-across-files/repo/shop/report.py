"""Sales report."""

from shop.tax import calc as tax_on


def tax_collected(carts):
    """Return the total tax across every cart."""

    return round(sum(tax_on(cart.subtotal()) for cart in carts), 2)
