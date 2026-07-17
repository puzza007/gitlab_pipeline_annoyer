# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A small Rust web service that receives GitLab pipeline webhooks and posts to a Slack channel when an MR pipeline fails, @-mentioning the MR author and merger. The entire application lives in `src/main.rs`.

## Commands

```shell
cargo build                 # build
cargo clippy                # lint (run before committing)
cargo fmt                   # format (run after clippy, before committing)
cargo run                   # run locally (requires env vars below)
```

Run tests with `cargo test` (there's a unit test that checks webhook parsing against `pipeline.json`). To exercise the webhook manually:

```shell
docker-compose up -d
curl -v -H 'Content-Type: application/json' -d @pipeline.json localhost:3000
```

`pipeline.json` is a sample GitLab pipeline webhook payload.

## Required environment variables

`GITLAB_API_TOKEN`, `GITLAB_API_HOSTNAME`, `SLACK_API_TOKEN`, `SLACK_CHANNEL`. Set `RUST_LOG=info` to see log output.

Dev mode: leaving `GITLAB_API_TOKEN` unset (or empty) connects to GitLab unauthenticated and skips the startup token check; `GITLAB_INSECURE=1` uses http instead of https. Together these allow smoke-testing the full webhook path against a local mock GitLab server without credentials (Slack still needs dummy values set and the final post will fail with `InvalidAuth` — everything before it can be verified from the logs).

## Architecture

Single axum server on port 3000 with one route: `POST /` handled by `webhook()` in `src/main.rs`. Flow:

1. Parse the body as JSON; only hooks with `object_kind: "pipeline"` and status `failed` are processed — everything else returns 200 and is skipped. The webhook payload and GitLab API response types are hand-rolled structs in `main.rs` (the `gitlab` crate removed its typed `webhooks`/`types` modules in 0.1706; clients define their own minimal `Deserialize` structs).
2. Look up the merge request for the pipeline's commit SHA via the GitLab API; pipelines without an MR are skipped.
3. Fetch the pipeline's jobs (paged, limit 300) and collect the failed ones.
4. Build a Slack message and post it via `slack_api` to `SLACK_CHANNEL`.

Slack user resolution (`get_slack_user_id`) assumes GitLab and Slack usernames match; if the Slack lookup fails it falls back to the raw username.

Shared state (`GitLab client, Slack client/token/channel`) is passed to the handler via an `Arc<State>` axum Extension.

Note: the `slack_api` dependency is a git fork (`puzza007/slack-rs-api`), not the crates.io release.

## Deployment

Pushes to `main` trigger GitHub Actions (`.github/workflows/build.yml`) to build the Dockerfile and push `puzza007/gitlab_pipeline_annoyer:latest` to DockerHub. The Dockerfile uses a two-stage build with a dependency-caching layer; `docker-compose.yml` runs the published image.
