# Generated system schema

`system-data-dictionary.json` and `baseline.json` are generated from code.
They are committed so consumers can read the current data dictionary without
building the workspace, and so compatibility changes are reviewable.

Never edit either file by hand. Regenerate them with:

```bash
scripts/schema.sh --update
scripts/schema.sh --update-baseline
```

`--update` rebuilds `system-data-dictionary.json` from
`obolargus_core::data_dictionary::export_system_schema()`. `--update-baseline`
promotes the current artifact to `baseline.json` after a deliberate
`x-obolargus-schema-version` bump.

`scripts/schema.sh --check` verifies the committed artifact matches a fresh
export and names the first differing definition when it does not.
