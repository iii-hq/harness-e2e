# judge-quality fixtures

The `judge_quality_*` scenarios embed these files (`src/scenarios/judge_quality/cases.rs`) and score a local judge provider's answers against their gold labels.

| File | Source | Revision | sha256 | Licence |
|---|---|---|---|---|
| `jevbench/easy.jsonl` | `fstandhartinger/jevbench` `datasets/public/easy.jsonl` | `609a664e` | `231df3c2c8e88a1a8c137ebe85de96ba70fabd330849098ac7b3c52c70b7172b` | MIT (per item) |
| `jevbench/original.jsonl` | same, `original.jsonl` | `609a664e` | `5c2414edb3006b8bfcb70fda433f0f9ca015759433849f8d3104328a1f7c4180` | MIT (repository) |
| `jevbench/hard.jsonl` | same, `hard.jsonl` | `609a664e` | `89e9e6becb33ed88c1de7d42dcc87531b2fb64cfaef4e1986faf7c37b3f80ebb` | MIT (per item) |
| `typed-decisions/test.jsonl` | Hugging Face `LocalLLaMA/typed-decisions` `all/test-00000-of-00001.parquet`, converted by `convert.py` | `e6aa632c4db73b59c7713bb21066ad1a8e1bfce6` | `5a9c27cf889f7cbf1ca09e439d6ae46722104eb9591ac2348b9f2194d38bbbd8` | Apache-2.0 |

Each directory carries its `LICENSE` and a `NOTICE` with the source and what was changed. The JevBench files are byte copies, so their sha256 equals the upstream `datasets/manifest.json`. `golden.json` is ours: hand-written cases and replies with no dataset text, and the metrics they must give.

## Shapes

- JevBench: one decision per line. `question` is a judge `Question` as `judge::evaluate` takes it; `expected` is a choice label, `yes`/`no`, or a score level. The scenario sends it under question id `q`.
- typed-decisions: one case of five questions per line, `{agree, gold, id, questions, state, workflow}`. A choice gold is its label, a noul gold a bool, a score gold an integer level. `agree` is the dataset's `label_agreement.argmax_agree`.

## Reading the numbers

- typed-decisions gold is the argmax of a teacher's distribution. The teacher confirms its own label on 1,188 of the 2,000 decisions (`agree`), and always answering the commonest label per workflow and question scores 0.5225. Read `accuracy_agreed` and `majority_accuracy` next to `accuracy`, and never pool this slice with JevBench. laya's typed-decisions checkpoint was trained on this dataset's train split.
- decider's training generators were written from the family names JevBench publishes, so JevBench favours decider. Its easy tier is near the ceiling for every published system; the hard tier separates them.

## Re-vendoring

A changed file is a new definition: put its sha256 in its `Slice` (`sha256` and `accuracy_description`) and the revision next to it. For typed-decisions, download the parquet at the new revision (any generic User-Agent, no token), update `PARQUET_SHA256` in `convert.py` and run:

```bash
uv run --with pyarrow python3 -I convert.py <test-00000-of-00001.parquet> test.jsonl
```

It checks the row and question counts, that every gold label answers its question and that every JSON object has sorted keys, and prints the output's sha256.
