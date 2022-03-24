# Build Stage
FROM rust:1.59.0 as builder

RUN USER=root cargo new --bin gitlab_pipeline_annoyer
WORKDIR ./gitlab_pipeline_annoyer
COPY ./Cargo.toml ./Cargo.toml
# Build empty app with downloaded dependencies to produce a stable image layer for next build
RUN cargo build --release

# Build web app with own code
RUN rm src/*.rs
ADD . ./
RUN rm ./target/release/deps/gitlab_pipeline_annoyer*
RUN cargo build --release


FROM debian:bullseye-slim
ARG APP=/usr/src/app

RUN apt-get update \
    && apt-get install -y ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*

EXPOSE 3000

ENV TZ=Pacific/Auckland \
    APP_USER=appuser

RUN groupadd $APP_USER \
    && useradd -g $APP_USER $APP_USER \
    && mkdir -p ${APP}

COPY --from=builder /gitlab_pipeline_annoyer/target/release/gitlab_pipeline_annoyer ${APP}/gitlab_pipeline_annoyer

RUN chown -R $APP_USER:$APP_USER ${APP}

USER $APP_USER
WORKDIR ${APP}

CMD ["./gitlab_pipeline_annoyer"]
