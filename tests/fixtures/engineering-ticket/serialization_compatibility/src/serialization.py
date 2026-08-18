def encode_event(event):
    return {"id": event["id"], "name": event["name"]}


def decode_event(payload):
    return {"id": payload["id"], "name": payload["name"]}
