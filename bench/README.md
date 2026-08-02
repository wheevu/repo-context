# repo-context benchmark methodology

This directory contains the benchmark runner for `repo-context export`. It records reproducible run metadata, but generated results remain ignored under `bench/results/` by default.

Run:

```bash
./bench/bench.sh          # benchmark this repository as the fixture
./bench/bench.sh <repo>   # benchmark another local repository fixture
```

The runner:

1. builds `repo-context` with `cargo build --release --locked`
2. measures the baseline `repo-context export --no-timestamp --mode rag`
3. builds a cold task index, then measures warm `--task --index-db` and
   `--task --no-index` exports
4. writes one raw timing JSON file per workflow
5. writes `metadata.json` with:
   - repo-context revision and dirty state
   - fixture path, revision, dirty state, file count, and content checksum
   - OS, machine, `rustc`, `cargo`, `hyperfine`, Python version
   - exact benchmark command
   - workflow names and raw timing-file paths

Generated files are intentionally not committed unless the project later adopts a fixture/result convention that makes them reproducible across machines. Do not publish Python-vs-Rust speedup claims without checking in both implementations, fixtures, exact commands, metadata, and raw results.
