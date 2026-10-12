"""Sales tax."""

STANDARD_RATE = 0.2


def compute_tax(amount, rate=STANDARD_RATE, *, rounding=2):
    """Return the tax due on `amount`, rounded to `rounding` decimal places."""

    return round(amount * rate, rounding)
