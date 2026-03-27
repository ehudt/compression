# Harness Ideas

Possible next steps for the autoresearch harness after the current campaign-loop
and logging-schema work.

## 1. Make campaign logging automatic

The current harness creates and validates `campaigns.tsv`, but it does not
force agents to populate it. The clean next step is to make campaign lifecycle
updates explicit and tool-backed.

Possible helper commands for `scripts/autoresearch_log.py`:

- `start-campaign`
- `update-campaign`
- `finish-campaign`
- `log-result`

That would let the skill call stable commands instead of relying on free-form
TSV edits.

## 2. Make `results.tsv` logging structured, not advisory

The schema now has richer columns:

- `campaign_id`
- `target`
- `axis`
- `levels`
- `rerun_status`
- `evidence_status`
- `per_file_notes`

But the harness still depends on the agent to fill them in correctly. A good
next step is to add a helper that appends rows with required fields and rejects
missing metadata.

## 3. Auto-populate `campaigns.tsv` from results

If agents reliably log `campaign_id`, `target`, `axis`, and `levels` in
`results.tsv`, the helper can derive or update `campaigns.tsv` automatically.

Useful behavior:

- create a campaign row when the first baseline row appears
- update campaign status when a keep/discard/blocked result lands
- attach the baseline commit automatically
- keep notes short and machine-friendly

## 4. Backfill historical metadata

`results.tsv` was migrated to the new schema, but old rows still have empty
metadata fields. A worthwhile cleanup pass would backfill at least:

- `campaign_id`
- `target`
- `axis`
- `levels`

This would make the new schema useful for trend analysis and allow stronger
automatic pivot rules.

## 5. Add stronger pivot enforcement

Right now pivot rules live in the skill instructions. They are not enforced by
code.

A reasonable next step:

- detect repeated discards in the same `campaign_id` or `target`
- warn after 2 related discards
- require a new campaign or explicit justification after 3

This should be based on structured metadata, not fuzzy parsing of experiment
descriptions.

## 6. Improve loop-health reporting

The loop now distinguishes agent completion from research yield, but reporting
is still lightweight.

Possible additions:

- count experiments per campaign baseline
- count keeps per target family
- count blocked and inconclusive runs separately from crash runs
- show the most recent campaign id in the loop summary

## 7. Update progress plotting

`scripts/plot_results_progress.py` still only uses a subset of `results.tsv`.
It could be extended to use the new metadata for:

- campaign-colored plots
- target-family filtering
- keep/discard rates by subsystem
- rerun-confirmed vs aggregate-only views

## 8. Preserve better evidence summaries

The harness still loses most per-file Silesia evidence once temp logs are gone.
A pragmatic next step would be to require short per-file summaries in
`per_file_notes` for every keep and every high-signal discard.

That keeps the committed log reviewable without storing large raw benchmark
artifacts in the repo.
