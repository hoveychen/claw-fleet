use std::convert::Infallible;
use std::time::Duration;

use async_stream::stream;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use serde::Deserialize;

use crate::app::AppState;
use crate::auth::ProjectPrincipal;
use crate::error::ApiError;
use crate::services::tasks;

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    after: Option<String>,
    project_id: Option<String>,
    task_id: Option<String>,
    types: Option<String>,
}

pub async fn stream_events(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    if query
        .project_id
        .as_deref()
        .is_some_and(|id| id != principal.project_id)
    {
        return Err(ApiError::PermissionDenied);
    }
    let resume = query.after.as_deref().or_else(|| {
        headers
            .get("last-event-id")
            .and_then(|value| value.to_str().ok())
    });
    let mut cursor = tasks::resolve_event_cursor(&state.pool, &principal, resume).await?;
    let event_types = query
        .types
        .map(|types| {
            types
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|types| !types.is_empty());
    let pool = state.pool.clone();
    let task_id = query.task_id;
    let output = stream! {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            match tasks::list_project_events(&pool, &principal, cursor, task_id.as_deref(), event_types.as_deref(), 200).await {
                Ok(events) => {
                    for item in events {
                        cursor = item.cursor.parse::<i64>().unwrap_or(cursor);
                        let data = match serde_json::to_string(&item) { Ok(data) => data, Err(_) => continue };
                        yield Ok(Event::default().id(item.cursor).event(item.event_type).data(data));
                    }
                }
                Err(_) => break,
            }
            interval.tick().await;
        }
    };
    Ok(Sse::new(output).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("heartbeat"),
    ))
}
