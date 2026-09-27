# Interchange schemas

`registry.json` is a machine-readable index of every versioned interchange
token (`openbnct.<name>/<version>`) declared by the Rust crates, with the
owning crate, module, and constant name. It is a checked mirror: a workspace
test (`catalogue::tests::schema_registry_mirrors_crate_constants`) scans
`crates/*/src/*.rs` and fails when the registry drifts. The serde contracts
in the crates remain the authoritative definitions.

Generated JSON Schema documents for cases, physical-dose bundles,
biological models, and evidence manifests will live here after their Rust
representations pass R0 review. Generated schemas must be checked against
golden valid and invalid documents before publication.
