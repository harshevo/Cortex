"""Sales tax."""

STANDARD_RATE = 0.2


def calc(amount, rate=STANDARD_RATE):
    """Return the tax due on `amount`, rounded to 2 decimal places."""

    return round(amount * rate, 2)
