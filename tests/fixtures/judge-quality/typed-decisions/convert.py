"""Convert the typed-decisions test parquet into the test.jsonl harness-e2e embeds.

Run once, by hand, never in CI:
    uv run --with pyarrow python3 -I convert.py <test-00000-of-00001.parquet> test.jsonl

One compact JSON line per case: {id, workflow, state (object), questions,
gold: {qid: label}, agree: {qid: bool}}. A choice gold is its label, a noul
gold a bool and a score gold an integer level. `agree` is the dataset's
label_agreement.argmax_agree. Teacher probabilities and factors are dropped.
"""

import hashlib
import json
import sys

import pyarrow.parquet as pq

PARQUET_SHA256 = "4f294f218ea1da27f3efef936359389c62ea4d3973a41457732990f1d31b647c"


def sorted_keys(pairs):
    keys = [key for key, _ in pairs]
    assert keys == sorted(keys), f"object keys are not sorted: {keys}"
    return dict(pairs)


def load(text):
    return json.loads(text, object_pairs_hook=sorted_keys)


def gold_of(question, label):
    kind = question["type"]
    if kind == "choice":
        assert label in question["criteria"], label
        return label
    if kind == "noul":
        assert label in ("true", "false"), label
        return label == "true"
    assert kind == "score", kind
    level = int(label)
    assert str(level) == label and 0 <= level < len(question["criteria"]), label
    return level


def main(source, target):
    data = open(source, "rb").read()
    assert hashlib.sha256(data).hexdigest() == PARQUET_SHA256, "unexpected parquet"
    rows = pq.read_table(source).to_pylist()
    assert len(rows) == 400, len(rows)
    lines, decisions = [], 0
    for row in rows:
        questions = load(row["questions"])
        gold = load(row["gold"])
        agreement = load(row["label_agreement"])
        assert sorted(gold) == sorted(questions) == sorted(agreement), row["id"]
        decisions += len(questions)
        case = {
            "agree": {qid: agreement[qid]["argmax_agree"] for qid in questions},
            "gold": {qid: gold_of(questions[qid], gold[qid]["label"]) for qid in questions},
            "id": row["id"],
            "questions": questions,
            "state": load(row["state"]),
            "workflow": row["workflow"],
        }
        lines.append(json.dumps(case, ensure_ascii=False, separators=(",", ":"), sort_keys=True))
    assert decisions == 2000, decisions
    output = ("\n".join(lines) + "\n").encode()
    open(target, "wb").write(output)
    print(f"sha256:{hashlib.sha256(output).hexdigest()} {len(output)} bytes")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
