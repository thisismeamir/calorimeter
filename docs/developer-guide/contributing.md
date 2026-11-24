# Contributing, Testing & Releases

## Local Verification Checklist

Before pushing or opening a PR, run the exact checks enforced by `.github/workflows/ci.yml`:

```bash
# 1. Formatting
cargo fmt --check

# 2. Zero-warning Clippy across all targets & feature combinations
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features calo-core/parquet -- -D warnings

# 3. Full test suite (default + Parquet feature)
cargo nextest run --workspace
cargo nextest run --workspace --features calo-core/parquet

# 4. Reference suite smoke check
cargo run -p calo-cli -- --suite examples/cosmolattice/suite.yaml --inventory examples/cosmolattice/inventory.yaml lint
cargo run -p calo-cli -- --suite examples/cosmolattice/suite.yaml --inventory examples/cosmolattice/inventory.yaml plan --format yaml
```

## Commit Message Style

Calorimeter uses short bracketed prefixes:
- `[impl]` — new feature or capability
- `[fix]` — bug fix or clippy/warning fix
- `[test]` — new or updated tests
- `[ci]` — workflow or build automation changes
- `[docs]` — documentation updates

## Building Docs Locally & Publishing to Read the Docs

```bash
pip install -r docs/requirements.txt
make -C docs html
# Open docs/_build/html/index.html
```

To publish on [readthedocs.org](https://readthedocs.org):
1. Copy `.readthedocs.yaml` to the repository root and `docs/` alongside `crates/`.
2. Push to GitHub and import the repository in Read the Docs.
3. Read the Docs automatically reads `.readthedocs.yaml`, installs `docs/requirements.txt`, and builds `docs/conf.py` on every push and release tag.

## Cutting a Tagged Release

Pushing a `v*` tag triggers `.github/workflows/release.yml`, which builds release archives for Linux (`x86_64`, `aarch64`), macOS (`arm64`, `x86_64`), and Windows (`x64`, `arm64`), generates `SHA256SUMS.txt`, and publishes the GitHub Release:

```bash
git tag -a v0.1.0 -m "Calorimeter v0.1.0"
git push origin v0.1.0
```
