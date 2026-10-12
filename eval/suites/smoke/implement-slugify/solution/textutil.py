"""Text helpers for building URLs."""

import re
import unicodedata


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

    decomposed = unicodedata.normalize("NFKD", text)
    plain = decomposed.encode("ascii", "ignore").decode("ascii")

    slug = re.sub(r"[^a-z0-9]+", "-", plain.lower()).strip("-")

    if max_length is not None:
        slug = slug[:max_length].rstrip("-")

    if not slug:
        raise ValueError("the text has no characters usable in a slug")

    return slug
