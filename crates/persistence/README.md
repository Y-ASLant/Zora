# How to perform migrations

## How do migrations work?

A SQLite database is a single file on the user's computer. When Zora starts,
`app/src/persistence/sqlite.rs` opens the database and runs the migrations
embedded by `crates/persistence` in a transaction.

Since we do not control the machine, migrations must be backward-compatible
with existing user databases and safe to run more than once.

## Step 1: One-time setup

Run `script/bootstrap` at least once. Local development dependencies include
`diesel_cli`; the repository's build-dependency script installs it with
`cargo binstall`.

The `sqlite3` binary is also useful during development. Use a recent version,
because some migrations rely on SQL features missing from older SQLite builds.

## Step 2: Write the migration

Run this command from the repository root:

```shell
diesel migration generate --migration-dir crates/persistence/migrations <descriptive-name>
```

This creates a directory containing `up.sql` and `down.sql`.

## Step 3: Run the migration and generate the schema

Replace `<path-to-warp.sqlite>` with the database used by the local Zora build:

```shell
diesel migration run \
  --migration-dir crates/persistence/migrations \
  --database-url="<path-to-warp.sqlite>"
diesel print-schema --database-url="<path-to-warp.sqlite>"
```

The generated schema lives at `crates/persistence/src/schema.rs`. Do not make
manual edits to that file in a change; use a migration and regenerate it.

## Reverting or redoing migrations

When iterating on a schema change, the migration can be reverted or rerun:

```shell
diesel migration revert \
  --migration-dir crates/persistence/migrations \
  --database-url="<path-to-warp.sqlite>"
diesel migration redo \
  --migration-dir crates/persistence/migrations \
  --database-url="<path-to-warp.sqlite>"
```

## Schema style

- Use `id` for integer primary keys unless a more descriptive primary-key name is needed.
- Use plural table names and singular Rust model names.
- If `bars` references `foos.id`, name the foreign-key column `foo_id`.

## The `schema.patch` file

`crates/persistence/schema.patch` contains the small manual adjustments that
are applied on top of Diesel's generated
`crates/persistence/src/schema.rs`. To refresh it:

1. Run the Diesel migrations and regenerate the schema.
2. Apply the required schema adjustment locally.
3. Run `git diff -U6 > crates/persistence/schema.patch`.

See the [Diesel patch-file documentation](https://diesel.rs/guides/configuring-diesel-cli.html#the-patch_file-field)
for background.
