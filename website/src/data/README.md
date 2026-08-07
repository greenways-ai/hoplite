# Website benchmark data

The checked-in JSON files are schema-v2 `pending` placeholders. They intentionally
contain no measurements.

The `HTTP and footprint benchmarks` workflow writes measured reports to
`benchmark-output/`, validates them with
`packaging/scripts/validate-benchmark-data.sh`, and uploads the stable
`hoplite-benchmarks` artifact.

The Pages workflow downloads and re-validates that artifact before copying it
into this directory for the site build. With no successful artifact, Pages keeps
the pending files and the website renders an explicit measurement-pending state.

Do not manually copy old benchmark numbers into these files or change `status`
to `measured` without running the validator.
