//! Database and repository integration tests on real SQLite.
//!
//! These live in one test binary because every binary under `tests/` links
//! the whole `rust_srec` library; separate files each pay that link after
//! every library edit. Add new suites as modules here rather than as new
//! top-level `tests/*.rs` files.

mod account_token_contracts;
mod credential_profile_contracts;
mod credential_profile_migration;
mod credential_profile_owner_deletion;
mod database_index_budget;
mod database_mutation_atomicity;
mod domain_boundaries;
mod filter_timezone_migration;
mod integration_tests;
mod job_claim_rank;
mod job_execution_logs_persist;
mod literal_database_search;
mod mesio_buffer_size_migration;
mod preset_template_timestamps;
mod proxy_route_migration;
mod retired_notification_subscription_migration;
mod sqlite_claim_stress;
mod timestamp_contract_inventory;
