import asyncio


class Operation:
    def __init__(self):
        self.resource_open = False
        self.state = "pending"

    async def run(self, started):
        self.resource_open = True
        self.state = "running"
        started.set()
        await asyncio.sleep(3600)
        self.resource_open = False
        self.state = "completed"
