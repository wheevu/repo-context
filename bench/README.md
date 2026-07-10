# repo-context benchmark methodology

This directory contains the benchmark runner for `repo-context export`. It records reproducible run metadata, but generated results remain ignored under `bench/results/` by default.

Run:

```bash
./bench/bench.sh          # benchmark this repository as the fixture
./bench/bench.sh <repo>   # benchmark another local repository fixture
```

The runner:

1. builds `repo-context` with `cargo build --release --locked`
2. runs `repo-context export --no-timestamp --mode rag` through `hyperfine`
3. writes raw `hyperfine.json`
4. writes `metadata.json` with:
   - repo-context revision and dirty state
   - fixture path, revision, dirty state, file count, and content checksum
   - OS, machine, `rustc`, `cargo`, `hyperfine`, Python version
   - exact benchmark command

Generated files are intentionally not committed unless the project later adopts a fixture/result convention that makes them reproducible across machines. Do not publish Python-vs-Rust speedup claims without checking in both implementations, fixtures, exact commands, metadata, and raw results.
