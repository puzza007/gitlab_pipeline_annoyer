use axum::{
    extract::rejection::JsonRejection, extract::Extension, http::StatusCode,
    response::IntoResponse, routing::post, Json, Router,
};

use gitlab::api::projects::merge_requests::MergeRequest;
use gitlab::api::projects::pipelines::PipelineJobs;
use gitlab::api::AsyncQuery;
use gitlab::types::Job as JobType;
use gitlab::types::MergeRequest as MergeRequestType;
use gitlab::webhooks::WebHook;
use gitlab::{AsyncGitlab, GitlabBuilder, StatusState};
use slack::chat::PostMessageRequest;
use slack::users::{InfoRequest, InfoResponse};
use slack_api as slack;
use slack_api::User;
use std::env;
use std::sync::Arc;
#[macro_use]
extern crate log;
use anyhow::{Context, Result};

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
    let gitlab_api_token = env::var("GITLAB_API_TOKEN").expect("Missing GITLAB_API_TOKEN env var");
    let slack_api_token = env::var("SLACK_API_TOKEN").expect("Missing SLACK_API_TOKEN env var");
    let slack_channel = env::var("SLACK_CHANNEL").expect("Missing SLACK_CHANNEL env var");
    let gitlab_api_hostname =
        env::var("GITLAB_API_HOSTNAME").expect("Missing GITLAB_API_HOSTNAME env var");

    info!("Connecting to gitlab...");
    let gitlab_client = GitlabBuilder::new(&gitlab_api_hostname, gitlab_api_token)
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
    payload: Result<Json<WebHook>, JsonRejection>,
    Extension(state): Extension<Arc<State>>,
) -> Result<impl IntoResponse, StatusCode> {
    match payload {
        Ok(Json(WebHook::Pipeline(pipelinehook))) => {
            info!("Pipeline received");
            let pipeline_id = pipelinehook.object_attributes.id.value();
            let project_id = pipelinehook.project.id.value();
            let pipeline_status = pipelinehook.object_attributes.status;
            info!("Pipeline status: {:?}", pipeline_status);

            if pipeline_status != StatusState::Failed {
                info!("Pipeline status not failure. Skipping.");
                return Ok(StatusCode::OK);
            }

            // Skip pipelines without an MR
            let merge_request = pipelinehook.merge_request.ok_or(StatusCode::OK)?;

            let merge_request_id = merge_request.id.value();

            let endpoint = PipelineJobs::builder()
                .project(project_id)
                .pipeline(pipeline_id)
                .build()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            let endpoint = gitlab::api::paged(endpoint, gitlab::api::Pagination::Limit(300));
            let jobs: Vec<JobType> = endpoint
                .query_async(&state.gitlab_client)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let mut failed = Vec::new();
            for job in jobs {
                if job.status != StatusState::Success {
                    failed.push((job.name.clone(), job.status, job.web_url.clone()));
                }
            }

            let merge_request_endpoint = MergeRequest::builder()
                .project(project_id)
                .merge_request(merge_request_id)
                .build()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            let merge_request: MergeRequestType = merge_request_endpoint
                .query_async(&state.gitlab_client)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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

            if let Some(merged_by) = merge_request.merged_by {
                let merged_by_slack_id = get_slack_user_id(&state, &merged_by.username).await;

                slack_message.push_str(format!("Merged by: <@{}>\n", merged_by_slack_id).as_str());
            }
            slack_message.push_str("Failed jobs\n");
            for (n, s, url) in failed {
                slack_message.push_str(format!("- <{}|{}> {:?}\n", url, n, s).as_str());
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
        Ok(_) => {
            info!("Not a pipeline. Skipping.");
            Ok(StatusCode::OK)
        }
        Err(err) => {
            error!("Got something unexpected {:?}", err);
            Err(StatusCode::OK)
        }
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
