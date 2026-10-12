"""Split a list of results into pages."""


def paginate(items, page, page_size):
    """Return the items on the given page.

    Pages are numbered from 1. A page past the end is an empty list.
    Raises ValueError if `page` or `page_size` is less than 1.
    """

    if page < 1 or page_size < 1:
        raise ValueError("page and page_size must be at least 1")

    start = page * page_size
    end = start + page_size

    return items[start:end]


def page_count(items, page_size):
    """Return how many pages are needed to show every item."""

    if page_size < 1:
        raise ValueError("page_size must be at least 1")

    return len(items) // page_size
