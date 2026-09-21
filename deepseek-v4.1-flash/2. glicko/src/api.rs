//! HTTP layer: routing, extractors and response shapes.

use std::sync::{Arc, RwLock};

use axum::extract::{DefaultBodyLimit, FromRequest, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::de::value::{MapAccessDeserializer, SeqAccessDeserializer};
use serde::de::{DeserializeOwned, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;

use crate::model::*;
use crate::store::{ImportReport, Store};

pub type AppState = Arc<RwLock<Store>>;

/// Import bodies can be large; the default 2 MiB limit is far too small.
const MAX_BODY_BYTES: usize = 256 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/events/import", post(import_events))
        .route("/api/v1/events", get(list_events))
        .route("/api/v1/leaderboard", get(leaderboard))
        .route("/api/v1/players/{login}", get(player_info))
        .route("/api/v1/players/{login}/matchups", get(player_matchups))
        .route("/api/v1/players/{login}/head-to-head/{opponent}", get(head_to_head))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    NotFound(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

fn read_store(state: &AppState) -> std::sync::RwLockReadGuard<'_, Store> {
    state.read().unwrap_or_else(|e| e.into_inner())
}

fn write_store(state: &AppState) -> std::sync::RwLockWriteGuard<'_, Store> {
    state.write().unwrap_or_else(|e| e.into_inner())
}

/// `Json` with rejections folded into the API's own error shape, so a malformed
/// body returns `400 {"error": "..."}` like every other failure instead of
/// axum's bare text/plain 4xx.
pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(ApiError::BadRequest(rejection.body_text())),
        }
    }
}

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListQuery {
    limit: Option<usize>,
    offset: Option<usize>,
    category: Option<String>,
    player: Option<String>,
}

impl ListQuery {
    /// `limit = 0` means "everything", which is what the unpaginated
    /// leaderboard uses.
    fn page(&self) -> Result<(usize, usize), ApiError> {
        let limit = self.limit.unwrap_or(DEFAULT_LIMIT);
        if limit > MAX_LIMIT {
            return Err(ApiError::BadRequest(format!(
                "limit must be 0 (all rows) or at most {MAX_LIMIT}"
            )));
        }
        Ok((limit, self.offset.unwrap_or(0)))
    }

    fn category(&self) -> Result<Category, ApiError> {
        Ok(self.category_opt()?.unwrap_or(Category::Total))
    }

    fn category_opt(&self) -> Result<Option<Category>, ApiError> {
        match self.category.as_deref() {
            None => Ok(None),
            Some(raw) => Category::parse(raw)
                .map(Some)
                .ok_or_else(|| ApiError::BadRequest(format!("unknown category {raw:?}; expected total, 1v1 or pub"))),
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination envelope
// ---------------------------------------------------------------------------

/// Response body for every list endpoint. `items` is borrowed from an `Arc` so
/// the cached leaderboard is never copied.
#[derive(Serialize)]
struct PageBody<'a, T> {
    items: &'a [T],
    total: usize,
    limit: usize,
    offset: usize,
}

/// A paginated response. `rows` is the whole collection and `range` selects the
/// page, so the cached leaderboard can be served without copying it.
pub struct Page<T> {
    rows: Arc<Vec<T>>,
    range: (usize, usize),
    total: usize,
    limit: usize,
    offset: usize,
}

impl<T> Page<T> {
    /// Page a complete, already-materialised collection.
    fn full(rows: Arc<Vec<T>>, total: usize, limit: usize, offset: usize) -> Self {
        let start = offset.min(rows.len());
        let end = if limit == 0 {
            rows.len()
        } else {
            start.saturating_add(limit).min(rows.len())
        };
        Self {
            rows,
            range: (start, end),
            total,
            limit,
            offset,
        }
    }

    /// Wrap rows that the query already paginated (they are still counted by
    /// `total`, which may be larger than the number of rows here).
    fn already_paginated(rows: Vec<T>, total: usize, limit: usize, offset: usize) -> Self {
        let end = rows.len();
        Self {
            rows: Arc::new(rows),
            range: (0, end),
            total,
            limit,
            offset,
        }
    }
}

impl<T: Serialize> IntoResponse for Page<T> {
    fn into_response(self) -> Response {
        let (start, end) = self.range;
        Json(PageBody {
            items: &self.rows[start..end],
            total: self.total,
            limit: self.limit,
            offset: self.offset,
        })
        .into_response()
    }
}

// ---------------------------------------------------------------------------
// Import payload
// ---------------------------------------------------------------------------

/// Accepts either `[{...}, {...}]` or `{"events": [{...}, {...}]}`.
#[derive(Debug)]
pub struct ImportBody(pub Vec<EventInput>);

impl<'de> Deserialize<'de> for ImportBody {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ImportVisitor;

        impl<'de> Visitor<'de> for ImportVisitor {
            type Value = ImportBody;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a list of events, or an object with an `events` list")
            }

            fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                Vec::<EventInput>::deserialize(SeqAccessDeserializer::new(seq)).map(ImportBody)
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                #[derive(Deserialize)]
                struct Wrapped {
                    events: Vec<EventInput>,
                }
                Wrapped::deserialize(MapAccessDeserializer::new(map)).map(|w| ImportBody(w.events))
            }
        }

        deserializer.deserialize_any(ImportVisitor)
    }
}

// ---------------------------------------------------------------------------
// Response shapes
// ---------------------------------------------------------------------------

#[derive(Serialize, Default)]
struct CategoryView {
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<StatsView>,
    #[serde(rename = "1v1", skip_serializing_if = "Option::is_none")]
    duel: Option<StatsView>,
    #[serde(rename = "pub", skip_serializing_if = "Option::is_none")]
    public: Option<StatsView>,
}

impl CategoryView {
    fn set(&mut self, cat: Category, stats: StatsView) {
        match cat {
            Category::Total => self.total = Some(stats),
            Category::Duel => self.duel = Some(stats),
            Category::Pub => self.public = Some(stats),
        }
    }
}

#[derive(Serialize)]
struct PlayerView {
    login: String,
    categories: CategoryView,
}

#[derive(Serialize)]
struct Side {
    login: String,
    kills: u64,
    deaths: u64,
    kdr: Option<f64>,
}

impl Side {
    fn new(login: &str, kills: u64, deaths: u64) -> Self {
        Self {
            login: login.to_string(),
            kills,
            deaths,
            kdr: if deaths == 0 {
                None
            } else {
                Some(kills as f64 / deaths as f64)
            },
        }
    }
}

#[derive(Serialize)]
struct HeadToHeadView {
    category: &'static str,
    player: Side,
    opponent: Side,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let store = read_store(&state);
    Json(json!({
        "status": "ok",
        "events": store.event_count(),
        "players": store.player_count(),
    }))
}

async fn import_events(State(state): State<AppState>, ApiJson(body): ApiJson<ImportBody>) -> Result<Json<ImportReport>, ApiError> {
    let ImportBody(events) = body;
    let mut store = write_store(&state);
    store.import(events).map(Json).map_err(ApiError::BadRequest)
}

async fn list_events(State(state): State<AppState>, Query(q): Query<ListQuery>) -> Result<Page<EventView>, ApiError> {
    let (limit, offset) = q.page()?;
    let (items, total) = read_store(&state).recent_events(q.player.as_deref(), limit, offset);
    Ok(Page::already_paginated(items, total, limit, offset))
}

async fn leaderboard(State(state): State<AppState>, Query(q): Query<ListQuery>) -> Result<Page<LeaderRow>, ApiError> {
    let cat = q.category()?;
    let (limit, offset) = q.page()?;
    let (rows, total) = read_store(&state).leaderboard(cat);
    Ok(Page::full(rows, total, limit, offset))
}

async fn player_info(
    State(state): State<AppState>,
    Path(login): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Json<PlayerView>, ApiError> {
    let wanted = q.category_opt()?;
    let store = read_store(&state);
    let id = store
        .player_id(&login)
        .ok_or_else(|| ApiError::NotFound(format!("unknown player {login:?}")))?;
    let stats = store.player_stats(id);
    drop(store);

    let mut categories = CategoryView::default();
    for cat in Category::ALL {
        if wanted.is_none_or(|c| c == cat) {
            categories.set(cat, stats[cat.index()].clone());
        }
    }
    Ok(Json(PlayerView { login, categories }))
}

async fn player_matchups(
    State(state): State<AppState>,
    Path(login): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Page<MatchupRow>, ApiError> {
    let cat = q.category()?;
    let (limit, offset) = q.page()?;
    let rows = {
        let store = read_store(&state);
        let id = store
            .player_id(&login)
            .ok_or_else(|| ApiError::NotFound(format!("unknown player {login:?}")))?;
        store.matchups(id, cat)
    };
    let total = rows.len();
    Ok(Page::already_paginated(
        rows.into_iter().skip(offset).take(if limit == 0 { usize::MAX } else { limit }).collect(),
        total,
        limit,
        offset,
    ))
}

async fn head_to_head(
    State(state): State<AppState>,
    Path((login, opponent)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> Result<Json<HeadToHeadView>, ApiError> {
    let cat = q.category()?;
    let store = read_store(&state);
    let a = store
        .player_id(&login)
        .ok_or_else(|| ApiError::NotFound(format!("unknown player {login:?}")))?;
    let b = store
        .player_id(&opponent)
        .ok_or_else(|| ApiError::NotFound(format!("unknown player {opponent:?}")))?;
    let (a_kills, b_kills) = store.head_to_head(a, b, cat).unwrap_or((0, 0));
    drop(store);

    Ok(Json(HeadToHeadView {
        category: cat.as_str(),
        player: Side::new(&login, a_kills, b_kills),
        opponent: Side::new(&opponent, b_kills, a_kills),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(json: &str) -> ImportBody {
        serde_json::from_str(json).expect("payload")
    }

    #[test]
    fn import_body_accepts_both_shapes() {
        let bare = body(r#"[{"id":"1","time":0,"killer":"a","victim":"b","player_count":2}]"#);
        let wrapped = body(r#"{"events":[{"id":"1","time":0,"killer":"a","victim":"b","player_count":2}]}"#);
        for ImportBody(events) in [bare, wrapped] {
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].killer, "a");
        }
    }

    #[test]
    fn import_body_reports_field_errors() {
        let err = serde_json::from_str::<ImportBody>(r#"[{"id":"1","time":0,"killer":"a"}]"#).unwrap_err();
        assert!(err.to_string().contains("victim"), "{err}");
    }

    #[test]
    fn timestamps_accept_seconds_millis_and_rfc3339() {
        let ImportBody(events) = body(
            r#"[{"id":"1","time":1700000000,"killer":"a","victim":"b","player_count":2},
                {"id":"2","time":1700000000000,"killer":"a","victim":"b","player_count":2},
                {"id":"3","time":"2023-11-14T22:13:20Z","killer":"a","victim":"b","player_count":2}]"#,
        );
        assert_eq!(events[0].time, 1_700_000_000);
        assert_eq!(events[1].time, 1_700_000_000);
        assert_eq!(events[2].time, 1_700_000_000);
    }

    #[test]
    fn paging_clamps_to_the_available_rows() {
        let rows = Arc::new(vec![1, 2, 3, 4, 5]);
        let page = Page::full(rows, 5, 2, 4);
        assert_eq!(page.range, (4, 5));
        let rows = Arc::new(vec![1, 2, 3]);
        let page = Page::full(rows, 3, 0, 0);
        assert_eq!(page.range, (0, 3), "limit 0 means everything");
        // Rows the query already paginated must not be re-sliced by offset.
        let page = Page::already_paginated(vec![1, 2, 3], 500, 3, 2);
        assert_eq!(page.range, (0, 3));
        assert_eq!(page.total, 500);
    }

    #[test]
    fn unknown_category_is_rejected() {
        let q = ListQuery {
            limit: None,
            offset: None,
            category: Some("ranked".into()),
            player: None,
        };
        assert!(q.category().is_err());
        let q = ListQuery {
            limit: Some(MAX_LIMIT + 1),
            offset: None,
            category: None,
            player: None,
        };
        assert!(q.page().is_err());
    }
}
