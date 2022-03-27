# Gitlab Pipeline Annoyer

## Purpose

Post to a Slack channel when a Gitlab MR pipeline fails, @ing both the
MR author and the merger.

The message will look something like

```
Failed MR: helm chart update for contact
Author: @john.smith
Merged by: @john.smith
Failed jobs
- test_dex Failed
- test_dev Failed
- test_smoke_dev Failed
```

## Configuration

Create a Slack [app](https://api.slack.com/apps/) and give it the following permissions

* chat:write
* chat:write.customize
* chat:write.public
* users.profile:read

## Running

The environment variables required are

* `GITLAB_API_TOKEN`
* `GITLAB_API_HOSTNAME`
* `SLACK_API_TOKEN`
* `SLACK_CHANNEL`

```shell
docker-compose up -d
```

## Testing

```shell
docker-compose up -d
curl -v -H 'Content-Type: application/json' -d @pipeline.json localhost:3000
```

## TODO

At the moment it is assumed your users have the same usernames in both
Gitlab and Slack
