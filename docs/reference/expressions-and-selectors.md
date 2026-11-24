# Expression Language & Selectors

## Expressions (`${{ ... }}`, `derive:`, `expr:`, `where:`)

Calorimeter implements a native, side-effect-free expression engine in `calo-core::expr` (host code is never evaluated; errors report exact byte spans).

### Literals & Operators
- **Literals:** integers, floats, strings, booleans (`true`/`false`), lists (`[1, 2, 3]`)
- **Arithmetic:** `+`, `-`, `*`, `/` (float division), `//` (floor division), `%`, `**` (integer exponent stays integer when non-negative; division by zero is an error)
- **Comparisons & logic:** `==`, `!=`, `<`, `<=`, `>`, `>=`, `and`, `or`, `not`, `in`
- **Conditional:** `a if cond else b`

### Built-In Functions
`min`, `max`, `abs`, `sqrt`, `cbrt`, `log`, `log2`, `log10`, `exp`, `floor`, `ceil`, `round`, `int`, `float`, `len`, `sum`

### Variable Namespaces
- `params.<key>` — benchmark parameter or derived axis value
- `machine.<path>` / `node.<path>` — inventory hardware fields
- `files.<name>` — rendered template file path from `step.files`
- `phase.<name>` — phase duration in seconds (from `CALO_CHANNEL`)
- `region.<name>.incl_s` — inclusive region duration in seconds
- `reported.<key>` — scalar metric emitted via `CALO_CHANNEL`
- Built-in metric names (e.g. `wall_s`, `wall_s.median`)

### Input File Templates (`{{ var }}`)

Templates referenced by `step.files.<name>.template` use strict `{{ var }}` substitution with no logic blocks. Missing variables are always errors (`R-STP-8`).

## Selectors (`select:`)

Used in `reports:` and `calo run --select`:

```yaml
select:
  benchmark: "strong-*"         # exact name or glob
  group: scaling
  params: { patch: none }       # exact equality on params
  where: "params.N >= 128"      # boolean expression
  mode: measure                 # measure | profile | kernel
  tier: [T1, M1]                # required active tiers
```

- Keys within a single selector **intersect** (AND).
- A YAML list of selectors is a **union** (OR).
- `select: all` matches every run.
