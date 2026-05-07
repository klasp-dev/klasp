# klasp gate JSON output schema

**Status**: stable at v0.3 forward.

`KLASP_OUTPUT_SCHEMA = 1` — see `klasp-core/src/protocol.rs`.

## Stability commitment

Within a v0.3.x release series:

- New fields may be **added** to any object (downstream tools must ignore
  unknown keys).
- Existing fields will **not be renamed or removed**.
- Field **ordering** at the same level is stable and can be relied upon.

A schema version bump (`KLASP_OUTPUT_SCHEMA = 2`) signals a breaking change
(removal, rename, or type change of an existing field). Breaking changes wait
for a major or minor release cycle and will be announced in `CHANGELOG.md`.

## Activate

```sh
klasp gate --format json
klasp gate --format json --output results.json
```

`--format json` writes to stdout (or `--output` path).
Terminal and JUnit/SARIF formatters are unaffected.

## Schema version (v1)

Every output document declares:

```json
{
  "output_schema_version": 1,
  "gate_schema_version": 2,
  ...
}
```

- `output_schema_version` — this document. Version `1` = the schema below.
  Increment only on breaking change.
- `gate_schema_version` — the stdin wire protocol version (`KLASP_GATE_SCHEMA`
  env var). Separate versioning axis; most gate upgrades do not change this.

## JSON shape (v1)

```json
{
  "output_schema_version": 1,
  "gate_schema_version": 2,
  "verdict": "pass | warn | fail",
  "checks": [
    {
      "name": "<string>",
      "source": "<string>",
      "verdict": "pass | warn | fail",
      "findings": [
        {
          "severity": "error | warn | info",
          "rule": "<string>",
          "file": "<string | null>",
          "line": "<number | null>",
          "message": "<string>"
        }
      ]
    }
  ],
  "stats": {
    "total_checks": 0,
    "pass": 0,
    "warn": 0,
    "fail": 0
  }
}
```

### Field reference

| Field | Type | Nullable | Description |
|---|---|---|---|
| `output_schema_version` | `u32` | no | Always `1` in v0.3.x. |
| `gate_schema_version` | `u32` | no | Wire protocol version (from `GATE_SCHEMA_VERSION`). |
| `verdict` | `"pass" \| "warn" \| "fail"` | no | Aggregate verdict across all checks. |
| `checks` | `array` | no | Per-check results. Empty when no checks ran. |
| `checks[].name` | `string` | no | Check name from `klasp.toml`. |
| `checks[].source` | `string` | no | Source identifier (e.g. `"shell"`, `"pre_commit"`, `"plugin:my-linter"`). |
| `checks[].verdict` | `"pass" \| "warn" \| "fail"` | no | Per-check outcome. |
| `checks[].findings` | `array` | no | Empty when verdict is `"pass"`. |
| `checks[].findings[].severity` | `"error" \| "warn" \| "info"` | no | Severity of this finding. |
| `checks[].findings[].rule` | `string` | no | Rule identifier (linter rule code, test name, etc.). |
| `checks[].findings[].file` | `string \| null` | yes | Source file path, or `null` if not applicable. |
| `checks[].findings[].line` | `number \| null` | yes | Line number within `file`, or `null` if not applicable. |
| `checks[].findings[].message` | `string` | no | Human-readable description of the finding. |
| `stats.total_checks` | `u32` | no | Total number of checks that ran. |
| `stats.pass` | `u32` | no | Checks with verdict `"pass"`. |
| `stats.warn` | `u32` | no | Checks with verdict `"warn"`. |
| `stats.fail` | `u32` | no | Checks with verdict `"fail"`. |

### Verdict semantics

- `"pass"` — all checks passed; gate allows the tool call.
- `"warn"` — at least one check issued warnings; gate allows the tool call but
  the agent is informed.
- `"fail"` — at least one check failed under the configured policy; gate blocks
  the tool call (process exits with code 2).

## Detecting schema drift in downstream tools

Read `output_schema_version` before parsing the rest of the document:

```python
import json, sys

doc = json.load(sys.stdin)
schema = doc["output_schema_version"]
if schema != 1:
    sys.exit(f"Unsupported KLASP_OUTPUT_SCHEMA {schema}; expected 1")
```

## Example outputs

### Pass (no checks ran)

```json
{
  "output_schema_version": 1,
  "gate_schema_version": 2,
  "verdict": "pass",
  "checks": [],
  "stats": {
    "total_checks": 0,
    "pass": 0,
    "warn": 0,
    "fail": 0
  }
}
```

### Warn (mixed results)

```json
{
  "output_schema_version": 1,
  "gate_schema_version": 2,
  "verdict": "warn",
  "checks": [
    {
      "name": "lint",
      "source": "shell",
      "verdict": "pass",
      "findings": []
    },
    {
      "name": "security",
      "source": "shell",
      "verdict": "warn",
      "findings": [
        {
          "severity": "warn",
          "rule": "dep-outdated",
          "file": null,
          "line": null,
          "message": "dependency is outdated"
        }
      ]
    }
  ],
  "stats": {
    "total_checks": 2,
    "pass": 1,
    "warn": 1,
    "fail": 0
  }
}
```

### Fail (with findings)

```json
{
  "output_schema_version": 1,
  "gate_schema_version": 2,
  "verdict": "fail",
  "checks": [
    {
      "name": "rustfmt",
      "source": "shell",
      "verdict": "fail",
      "findings": [
        {
          "severity": "error",
          "rule": "fmt",
          "file": "src/lib.rs",
          "line": 10,
          "message": "not formatted"
        }
      ]
    }
  ],
  "stats": {
    "total_checks": 1,
    "pass": 0,
    "warn": 0,
    "fail": 1
  }
}
```
