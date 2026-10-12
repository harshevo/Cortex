"""Text helpers for building URLs."""


def slugify(text, max_length=None):
    """Turn `text` into a URL slug.

    Rules, applied in this order:

    1. Accented letters are reduced to their plain form ("é" becomes "e").
       Characters with no ASCII form are dropped.
    2. Letters are lower-cased.
    3. Every run of characters other than a-z and 0-9 becomes one hyphen.
    4. Leading and trailing hyphens are removed.
    5. If `max_length` is given, the slug is cut to at most that many
       characters, and a hyphen left at the end by the cut is removed.

    Raises ValueError if the resulting slug is empty.

    Examples:

        slugify("Hello, World!")                  -> "hello-world"
        slugify("Crème Brûlée")                   -> "creme-brulee"
        slugify("hello world again", max_length=6) -> "hello"
    """

    raise NotImplementedError
