"""Append-only event store used by the feature fixture."""


class EventStore:
    def __init__(self, events):
        self.events = list(events)

    def after(self, cursor, limit):
        start = 0 if cursor is None else cursor + 1
        page = self.events[start : start + limit]
        next_cursor = None if start + len(page) >= len(self.events) else start + len(page) - 1
        return page, next_cursor
