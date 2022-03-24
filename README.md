# Gitlab Pipeline Annoyer

## Purpose

To alert the person who created a merge request that the associated CI
pipeline has failed

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
