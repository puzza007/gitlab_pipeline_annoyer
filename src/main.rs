use axum::{
    extract::rejection::JsonRejection, extract::Extension, http::StatusCode,
    response::IntoResponse, routing::post, Json, Router,
};

use gitlab::api::projects::pipelines::PipelineJobs;
use gitlab::api::projects::repository::commits::MergeRequests;
use gitlab::api::AsyncQuery;
use gitlab::{AsyncGitlab, GitlabBuilder};
use serde::Deserialize;
use slack::chat::PostMessageRequest;
use slack::users::{InfoRequest, InfoResponse};
use slack_api as slack;
use slack_api::User;
use std::env;
use std::sync::Arc;
#[macro_use]
extern crate log;
use anyhow::{Context, Result};

// The gitlab crate no longer ships webhook payload or API response types;
// clients are expected to define structs for the fields they use.
#[derive(Debug, Deserialize)]
struct PipelineHook {
    object_attributes: PipelineAttributes,
    project: Project,
    commit: Option<Commit>,
}

#[derive(Debug, Deserialize)]
struct PipelineAttributes {
    id: u64,
    status: String,
}

#[derive(Debug, Deserialize)]
struct Project {
    id: u64,
}

#[derive(Debug, Deserialize)]
struct Commit {
    id: String,
}

#[derive(Debug, Deserialize)]
struct Job {
    name: String,
    status: String,
    web_url: String,
}

#[derive(Debug, Deserialize)]
struct MergeRequest {
    title: String,
    web_url: String,
    author: UserBrief,
    merged_by: Option<UserBrief>,
}

#[derive(Debug, Deserialize)]
struct UserBrief {
    username: String,
}

struct State {
    gitlab_client: AsyncGitlab,
    slack_client: reqwest::Client,
    slack_token: String,
    slack_channel: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    info!("Reading env vars...");
    let gitlab_api_token = env::var("GITLAB_API_TOKEN").unwrap_or_default();
    let slack_api_token = env::var("SLACK_API_TOKEN").expect("Missing SLACK_API_TOKEN env var");
    let slack_channel = env::var("SLACK_CHANNEL").expect("Missing SLACK_CHANNEL env var");
    let gitlab_api_hostname =
        env::var("GITLAB_API_HOSTNAME").expect("Missing GITLAB_API_HOSTNAME env var");

    info!("Connecting to gitlab...");
    let mut gitlab_builder = if gitlab_api_token.is_empty() {
        warn!("GITLAB_API_TOKEN not set; connecting to gitlab unauthenticated (dev mode)");
        GitlabBuilder::new_unauthenticated(&gitlab_api_hostname)
    } else {
        GitlabBuilder::new(&gitlab_api_hostname, gitlab_api_token)
    };
    if env::var("GITLAB_INSECURE").is_ok_and(|v| v == "1") {
        warn!("GITLAB_INSECURE=1; using http instead of https");
        gitlab_builder.insecure();
    }
    let gitlab_client = gitlab_builder
        .build_async()
        .await
        .context(format!("Couldn't connect to gitlab: {gitlab_api_hostname}"))?;

    let slack_client = slack::default_client().unwrap();

    let shared_state = Arc::new(State {
        gitlab_client,
        slack_client,
        slack_token: slack_api_token.to_string(),
        slack_channel,
    });

    let app = Router::new()
        .route("/", post(webhook))
        .layer(Extension(shared_state));

    info!("Starting web server...");
    axum::Server::bind(&"0.0.0.0:3000".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .context("Couldn't start server on 0.0.0.0:3000")?;

    Ok(())
}

async fn webhook(
    Extension(state): Extension<Arc<State>>,
    payload: Result<Json<serde_json::Value>, JsonRejection>,
) -> Result<impl IntoResponse, StatusCode> {
    match payload {
        Ok(Json(value)) => {
            if value.get("object_kind").and_then(|k| k.as_str()) != Some("pipeline") {
                info!("Not a pipeline. Skipping.");
                return Ok(StatusCode::OK);
            }

            let pipelinehook: PipelineHook = serde_json::from_value(value).map_err(|err| {
                error!("Couldn't parse pipeline hook: {:?}", err);
                StatusCode::OK
            })?;

            let pipeline_id = pipelinehook.object_attributes.id;
            let project_id = pipelinehook.project.id;
            let pipeline_status = pipelinehook.object_attributes.status;
            info!("Pipeline {} received", pipeline_id);
            info!("Pipeline status: {}", pipeline_status);

            if pipeline_status != "failed" {
                info!("Pipeline status not failure. Skipping.");
                return Ok(StatusCode::OK);
            }

            info!("Checking if pipeline has an MR");

            let commit_id = pipelinehook.commit.ok_or(StatusCode::OK)?.id;
            let commit_merge_requests_endpoint = MergeRequests::builder()
                .project(project_id)
                .sha(commit_id)
                .build()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            let commit_merge_requests: Vec<MergeRequest> = commit_merge_requests_endpoint
                .query_async(&state.gitlab_client)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            let merge_request = commit_merge_requests.first().ok_or(StatusCode::OK)?;

            info!("Pipeline has an MR");

            let endpoint = PipelineJobs::builder()
                .project(project_id)
                .pipeline(pipeline_id)
                .build()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            let endpoint = gitlab::api::paged(endpoint, gitlab::api::Pagination::Limit(300));
            let jobs: Vec<Job> = endpoint
                .query_async(&state.gitlab_client)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let mut failed = Vec::new();
            for job in jobs {
                if job.status == "failed" {
                    failed.push((job.name, job.status, job.web_url));
                }
            }

            let mut slack_message = String::new();
            slack_message.push_str(
                format!(
                    "Failed MR: <{}|{}>\n",
                    merge_request.web_url, merge_request.title
                )
                .as_str(),
            );

            let author_slack_id = get_slack_user_id(&state, &merge_request.author.username).await;
            slack_message.push_str(format!("Author: <@{}>\n", author_slack_id).as_str());

            if let Some(merged_by) = &merge_request.merged_by {
                let merged_by_slack_id = get_slack_user_id(&state, &merged_by.username).await;

                slack_message.push_str(format!("Merged by: <@{}>\n", merged_by_slack_id).as_str());
            }
            slack_message.push_str("Failed jobs\n");
            for (n, s, url) in failed {
                slack_message.push_str(format!("- <{}|{}> {}\n", url, n, s).as_str());
            }

            let slack_client = &state.slack_client;
            let slack_token = &state.slack_token;
            let message_request = PostMessageRequest {
                channel: &state.slack_channel,
                text: &slack_message,
                ..PostMessageRequest::default()
            };

            slack::chat::post_message(slack_client, slack_token, &message_request)
                .await
                .map_err(|e| {
                    error!("Slack error {:?}", (e, slack_token, message_request));
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
            info!("Slacked: {}", &slack_message);

            Ok(StatusCode::OK)
        }
        Err(err) => {
            error!("Got something unexpected {:?}", err);
            Err(StatusCode::OK)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PipelineHook;

    #[test]
    fn parses_sample_pipeline_hook() {
        let payload = include_str!("../pipeline.json");
        let hook: PipelineHook = serde_json::from_str(payload).unwrap();
        assert_eq!(hook.object_attributes.status, "failed");
        assert_eq!(hook.project.id, 3575);
        assert!(hook.commit.is_some());
    }
}

async fn get_slack_user_id(state: &State, username: &str) -> String {
    let author_info_request = InfoRequest { user: username };
    match slack::users::info(
        &state.slack_client,
        &state.slack_token,
        &author_info_request,
    )
    .await
    {
        Ok(InfoResponse {
            user: Some(User { id: Some(id), .. }),
            ..
        }) => id,
        _ => username.to_string(),
    }
}
