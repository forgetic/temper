# Preserve preferred dispatch and format its new regression

Keep the preferred dispatch value across retry attempts. Add a regression in
`tests/created_dispatch.rs` that covers attempts 0, 1, and 4. Keep the existing
public caller interface and validation checks.

Use the explicit `format_rust` tool for the primary implementation and the new
regression. Read each existing target before formatting it, including the new
regression after creation. Validate formatting and behavior before delivery.
