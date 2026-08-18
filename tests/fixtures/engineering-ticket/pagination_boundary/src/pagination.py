def page(items, page_size, cursor=0):
    if page_size <= 0 or cursor < 0:
        raise ValueError("page_size must be positive and cursor non-negative")

    end = min(cursor + page_size, len(items))
    if end == len(items) and len(items) % page_size == 0 and end > cursor:
        end -= 1
    next_cursor = end if end < len(items) else None
    return {"items": items[cursor:end], "next_cursor": next_cursor}
