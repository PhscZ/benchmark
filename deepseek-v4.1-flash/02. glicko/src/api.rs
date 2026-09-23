//! HTTP layer: extractors, DTOs and handlers.
//!
//! Two families of endpoints share the same handlers: the default ones collapse
//! alt accounts into their main, the `/noalt/` ones treat every login as its own
//! player. Both read from the same recomputed state, so they can never disagree.

use crate::config::Config;
use crate::engine::CatStats;
use crate::error::AppError;
use crate::glicko::{kd_ratio, winrate};
use crate::model::{AltEntry, Category, EventInput, StoredEvent};
use crate::store::{ImportReport, Mode, Store};
use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use utoipa::{IntoParams, ToSchema};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<RwLock<Store>>,
    pub config: Arc<Config>,
}

impl AppState {
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, Store> {
        self.store
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn write(&self) -> std::sync::RwLockWriteGuard<'_, Store> {
        self.store
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// `Json`, but malformed bodies come back as the same `{"error": ...}` shape as
/// every other failure instead of axum's plain-text rejection.
pub struct ApiJson<T>(pub T);

impl<S, T> axum::extract::FromRequest<S> for ApiJson<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(
        request: axum::extract::Request,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(request, state)
            .await
            .map_err(|rejection| AppError::BadRequest(rejection.body_text()))?;
        Ok(ApiJson(value))
    }
}

// ---------------------------------------------------------------- query types

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CategoryQuery {
    /// Rating ladder: `total`, `1v1` or `pub`. Defaults to `total`.
    pub category: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    /// 1-based page number.
    pub page: Option<u64>,
    /// Items per page (max configured by `--max-page-size`).
    pub limit: Option<u64>,
    /// Rating ladder: `total`, `1v1` or `pub`. Defaults to `total`.
    pub category: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EventsQuery {
    /// 1-based page number.
    pub page: Option<u64>,
    /// Items per page (max configured by `--max-page-size`).
    pub limit: Option<u64>,
    /// Only events where this login was the killer or the victim.
    pub player: Option<String>,
}

pub struct Pagination {
    pub page: usize,
    pub limit: usize,
    pub offset: usize,
}

impl Pagination {
    pub fn new(page: Option<u64>, limit: Option<u64>, config: &Config) -> Result<Self, AppError> {
        let page = page.unwrap_or(1);
        if page == 0 {
            return Err(AppError::BadRequest("page must be at least 1".into()));
        }
        let limit = limit.unwrap_or(config.default_page_size as u64);
        if limit == 0 {
            return Err(AppError::BadRequest("limit must be at least 1".into()));
        }
        if limit > config.max_page_size as u64 {
            return Err(AppError::BadRequest(format!(
                "limit must not exceed {}",
                config.max_page_size
            )));
        }
        Ok(Self {
            page: page as usize,
            limit: limit as usize,
            offset: (page as usize - 1) * limit as usize,
        })
    }

    pub fn total_pages(&self, total: u64) -> u64 {
        total.div_ceil(self.limit as u64)
    }
}

pub fn category_of(value: Option<&str>) -> Result<Category, AppError> {
    match value {
        None => Ok(Category::Total),
        Some(value) => Category::parse(value).ok_or_else(|| {
            AppError::BadRequest(format!(
                "unknown category `{value}`, expected one of: total, 1v1, pub"
            ))
        }),
    }
}

// ----------------------------------------------------------------------- DTOs

#[derive(Debug, Serialize, ToSchema)]
pub struct LeaderboardEntry {
    /// 1-based position in the ladder.
    pub rank: u64,
    pub login: String,
    pub rating: f64,
    pub rd: f64,
    pub wins: u64,
    pub losses: u64,
    /// Wins over played matches, 0.0 when nothing has been played.
    pub winrate: f64,
    /// Cumulative kills; never reset when a race ends.
    pub kills: u64,
    /// Cumulative deaths; never reset when a race ends.
    pub deaths: u64,
    /// Kills per death; a player with no deaths is credited with their kills.
    pub kd_ratio: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LeaderboardPage {
    pub page: u64,
    pub limit: u64,
    pub total: u64,
    pub total_pages: u64,
    pub items: Vec<LeaderboardEntry>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PlayerInfo {
    /// Canonical login: the main account on alt-aware endpoints.
    pub login: String,
    pub category: String,
    pub rating: f64,
    pub rd: f64,
    /// Current volatility (sigma) of the rating.
    pub volatility: f64,
    /// Timestamp of the player's last eligible event, or null when they have no
    /// history in this category.
    pub last_active: Option<DateTime<Utc>>,
    pub wins: u64,
    pub losses: u64,
    pub winrate: f64,
    pub kills: u64,
    pub deaths: u64,
    pub kd_ratio: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MatchupEntry {
    pub opponent: String,
    pub kills: u64,
    pub deaths: u64,
    pub kd_ratio: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MatchupPage {
    pub page: u64,
    pub limit: u64,
    pub total: u64,
    pub total_pages: u64,
    pub items: Vec<MatchupEntry>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HeadToHead {
    pub login: String,
    pub opponent: String,
    pub category: String,
    pub kills: u64,
    pub deaths: u64,
    pub kd_ratio: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EventDto {
    pub id: String,
    pub time: DateTime<Utc>,
    pub player_count: u32,
    /// Killer as recorded in the source system.
    pub killer: String,
    /// Victim as recorded in the source system.
    pub victim: String,
    /// Killer after alt resolution (identical to `killer` on `/noalt/`).
    pub killer_resolved: String,
    /// Victim after alt resolution (identical to `victim` on `/noalt/`).
    pub victim_resolved: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EventPage {
    pub page: u64,
    pub limit: u64,
    pub total: u64,
    pub total_pages: u64,
    pub items: Vec<EventDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AltsResponse {
    pub total: u64,
    pub items: Vec<AltEntry>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AltsRequest {
    /// Alt-account mappings to add or replace.
    pub items: Vec<AltEntry>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub events: u64,
    pub players: u64,
    pub matches: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConfigResponse {
    pub initial_rating: f64,
    pub initial_rd: f64,
    pub initial_volatility: f64,
    pub scale: f64,
    pub epsilon: f64,
    pub tau: f64,
    pub match_kill_target: u32,
    /// Logins excluded from all rating maths and statistics.
    pub excluded_logins: Vec<String>,
    pub default_page_size: usize,
    pub max_page_size: usize,
    pub events_file: String,
    pub alts_file: String,
}

// ------------------------------------------------------------------ handlers

#[utoipa::path(
    get,
    path = "/health",
    tag = "service",
    responses((status = 200, description = "Liveness and log size", body = HealthResponse))
)]
pub async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    let store = state.read();
    let stats = store.compute_stats(Mode::Alt);
    Json(HealthResponse {
        status: "ok".into(),
        events: store.events().len() as u64,
        players: store.state(Mode::Alt).registered as u64,
        matches: stats.matches,
    })
}

#[utoipa::path(
    get,
    path = "/config",
    tag = "service",
    responses((status = 200, description = "Effective configuration", body = ConfigResponse))
)]
pub async fn get_config(State(state): State<AppState>) -> Json<ConfigResponse> {
    let store = state.read();
    let params = *store.params();
    let mut excluded: Vec<String> = store.excluded().iter().cloned().collect();
    excluded.sort();
    Json(ConfigResponse {
        initial_rating: params.initial_rating,
        initial_rd: params.initial_rd,
        initial_volatility: params.initial_volatility,
        scale: params.scale,
        epsilon: params.epsilon,
        tau: params.tau,
        match_kill_target: crate::engine::MATCH_KILL_TARGET,
        excluded_logins: excluded,
        default_page_size: state.config.default_page_size,
        max_page_size: state.config.max_page_size,
        events_file: store.events_path().display().to_string(),
        alts_file: store.alts_path().display().to_string(),
    })
}

/// Imports kill events. Ids are stable, so re-posting the same file is a no-op.
#[utoipa::path(
    post,
    path = "/events/import",
    tag = "events",
    request_body = Vec<EventInput>,
    responses(
        (status = 200, description = "Import report", body = ImportReport),
        (status = 400, description = "Malformed payload", body = crate::error::ErrorResponse)
    )
)]
pub async fn import_events(
    State(state): State<AppState>,
    ApiJson(events): ApiJson<Vec<EventInput>>,
) -> Result<Json<ImportReport>, AppError> {
    let mut store = state.write();
    let report = store.import_events(events)?;
    tracing::info!(
        imported = report.imported,
        duplicates = report.duplicates,
        total = report.total_events,
        "events imported"
    );
    Ok(Json(report))
}

#[utoipa::path(
    get,
    path = "/alts",
    tag = "alts",
    responses((status = 200, description = "Known alt mappings", body = AltsResponse))
)]
pub async fn list_alts(State(state): State<AppState>) -> Json<AltsResponse> {
    let store = state.read();
    Json(AltsResponse {
        total: store.alts().len() as u64,
        items: store.alts().values().cloned().collect(),
    })
}

/// Adds or replaces alt mappings and recalculates every rating from the log.
#[utoipa::path(
    post,
    path = "/alts",
    tag = "alts",
    request_body = AltsRequest,
    responses(
        (status = 200, description = "Recalculation report", body = ImportReport),
        (status = 400, description = "Invalid mapping", body = crate::error::ErrorResponse)
    )
)]
pub async fn add_alts(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<AltsRequest>,
) -> Result<Json<ImportReport>, AppError> {
    let mut store = state.write();
    let report = store.upsert_alts(request.items)?;
    Ok(Json(report))
}

/// Removes a mapping; the account becomes a player of its own again.
#[utoipa::path(
    delete,
    path = "/alts/{alt}",
    tag = "alts",
    params(("alt" = String, Path, description = "Alt login to unmap")),
    responses(
        (status = 200, description = "Recalculation report", body = ImportReport),
        (status = 404, description = "Unknown alt", body = crate::error::ErrorResponse)
    )
)]
pub async fn delete_alt(
    State(state): State<AppState>,
    Path(alt): Path<String>,
) -> Result<Json<ImportReport>, AppError> {
    let mut store = state.write();
    match store.remove_alt(&alt)? {
        Some(report) => Ok(Json(report)),
        None => Err(AppError::NotFound(format!("no alt mapping for `{alt}`"))),
    }
}

// --- shared response builders -----------------------------------------------

fn leaderboard_entry(rank: u64, login: &str, stats: &CatStats) -> LeaderboardEntry {
    LeaderboardEntry {
        rank,
        login: login.to_string(),
        rating: stats.rating,
        rd: stats.rd,
        wins: stats.wins,
        losses: stats.losses,
        winrate: winrate(stats.wins, stats.losses),
        kills: stats.kills,
        deaths: stats.deaths,
        kd_ratio: kd_ratio(stats.kills, stats.deaths),
    }
}

fn full_leaderboard(store: &Store, mode: Mode, category: Category) -> Vec<LeaderboardEntry> {
    store
        .leaderboard(mode, category)
        .iter()
        .enumerate()
        .map(|(rank, &id)| {
            let stats = store
                .state(mode)
                .stats(id, category)
                .copied()
                .unwrap_or_default();
            leaderboard_entry(rank as u64 + 1, store.name(id), &stats)
        })
        .collect()
}

fn paged_leaderboard(
    store: &Store,
    mode: Mode,
    category: Category,
    page: &Pagination,
) -> LeaderboardPage {
    let board = store.leaderboard(mode, category);
    let total = board.len() as u64;
    let items = board
        .iter()
        .skip(page.offset)
        .take(page.limit)
        .enumerate()
        .map(|(index, &id)| {
            let stats = store
                .state(mode)
                .stats(id, category)
                .copied()
                .unwrap_or_default();
            leaderboard_entry((page.offset + index + 1) as u64, store.name(id), &stats)
        })
        .collect();
    LeaderboardPage {
        page: page.page as u64,
        limit: page.limit as u64,
        total,
        total_pages: page.total_pages(total),
        items,
    }
}

fn player_info(
    store: &Store,
    mode: Mode,
    login: &str,
    category: Category,
) -> Result<PlayerInfo, AppError> {
    let (id, stats) = store
        .player_stats(mode, login, category)
        .ok_or_else(|| AppError::NotFound(format!("unknown player `{login}`")))?;
    Ok(PlayerInfo {
        login: store.name(id).to_string(),
        category: category.as_str().to_string(),
        rating: stats.rating,
        rd: stats.rd,
        volatility: stats.volatility,
        last_active: stats
            .last_active
            .and_then(|secs| DateTime::from_timestamp(secs, 0)),
        wins: stats.wins,
        losses: stats.losses,
        winrate: winrate(stats.wins, stats.losses),
        kills: stats.kills,
        deaths: stats.deaths,
        kd_ratio: kd_ratio(stats.kills, stats.deaths),
    })
}

fn matchup_page(
    store: &Store,
    mode: Mode,
    login: &str,
    category: Category,
    page: &Pagination,
) -> Result<MatchupPage, AppError> {
    let list = store
        .matchups(mode, login, category)
        .ok_or_else(|| AppError::NotFound(format!("unknown player `{login}`")))?;
    let total = list.len() as u64;
    let items = list
        .iter()
        .skip(page.offset)
        .take(page.limit)
        .map(|(opponent, stats)| MatchupEntry {
            opponent: store.name(*opponent).to_string(),
            kills: stats.kills,
            deaths: stats.deaths,
            kd_ratio: kd_ratio(stats.kills, stats.deaths),
        })
        .collect();
    Ok(MatchupPage {
        page: page.page as u64,
        limit: page.limit as u64,
        total,
        total_pages: page.total_pages(total),
        items,
    })
}

fn head_to_head(
    store: &Store,
    mode: Mode,
    login: &str,
    opponent: &str,
    category: Category,
) -> Result<HeadToHead, AppError> {
    let (player_id, opponent_id, stats) = store
        .head_to_head(mode, login, opponent, category)
        .ok_or_else(|| AppError::NotFound(format!("unknown player `{login}` or `{opponent}`")))?;
    Ok(HeadToHead {
        login: store.name(player_id).to_string(),
        opponent: store.name(opponent_id).to_string(),
        category: category.as_str().to_string(),
        kills: stats.kills,
        deaths: stats.deaths,
        kd_ratio: kd_ratio(stats.kills, stats.deaths),
    })
}

fn event_dto(store: &Store, mode: Mode, event: &StoredEvent) -> EventDto {
    let state = store.state(mode);
    EventDto {
        id: event.id.clone(),
        time: event.time,
        player_count: event.player_count,
        killer: store.name(event.killer).to_string(),
        victim: store.name(event.victim).to_string(),
        killer_resolved: store.name(state.canonical(event.killer)).to_string(),
        victim_resolved: store.name(state.canonical(event.victim)).to_string(),
    }
}

fn event_page(
    store: &Store,
    mode: Mode,
    query: &EventsQuery,
    page: &Pagination,
) -> Result<EventPage, AppError> {
    let player = match query.player.as_deref() {
        None => None,
        Some(login) => Some(
            store
                .resolve_login(mode, login)
                .ok_or_else(|| AppError::NotFound(format!("unknown player `{login}`")))?,
        ),
    };
    let (events, total) = store.events_page(mode, player, page.offset, page.limit);
    Ok(EventPage {
        page: page.page as u64,
        limit: page.limit as u64,
        total,
        total_pages: page.total_pages(total),
        items: events
            .into_iter()
            .map(|event| event_dto(store, mode, event))
            .collect(),
    })
}

// --- alt-aware endpoints ----------------------------------------------------

#[utoipa::path(
    get,
    path = "/leaderboard",
    tag = "leaderboard",
    params(CategoryQuery),
    responses(
        (status = 200, description = "Complete ladder, alt accounts collapsed", body = Vec<LeaderboardEntry>),
        (status = 400, description = "Invalid category", body = crate::error::ErrorResponse)
    )
)]
pub async fn leaderboard(
    State(state): State<AppState>,
    Query(query): Query<CategoryQuery>,
) -> Result<Json<Vec<LeaderboardEntry>>, AppError> {
    let category = category_of(query.category.as_deref())?;
    let store = state.read();
    Ok(Json(full_leaderboard(&store, Mode::Alt, category)))
}

#[utoipa::path(
    get,
    path = "/leaderboard/paginated",
    tag = "leaderboard",
    params(PageQuery),
    responses(
        (status = 200, description = "One page of the ladder, alt accounts collapsed", body = LeaderboardPage),
        (status = 400, description = "Invalid pagination or category", body = crate::error::ErrorResponse)
    )
)]
pub async fn leaderboard_paginated(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Json<LeaderboardPage>, AppError> {
    let category = category_of(query.category.as_deref())?;
    let page = Pagination::new(query.page, query.limit, &state.config)?;
    let store = state.read();
    Ok(Json(paged_leaderboard(&store, Mode::Alt, category, &page)))
}

#[utoipa::path(
    get,
    path = "/players/{login}",
    tag = "players",
    params(
        ("login" = String, Path, description = "Player login (alt accounts resolve to their main)"),
        CategoryQuery
    ),
    responses(
        (status = 200, description = "Player statistics", body = PlayerInfo),
        (status = 400, description = "Invalid category", body = crate::error::ErrorResponse),
        (status = 404, description = "Unknown player", body = crate::error::ErrorResponse)
    )
)]
pub async fn player(
    State(state): State<AppState>,
    Path(login): Path<String>,
    Query(query): Query<CategoryQuery>,
) -> Result<Json<PlayerInfo>, AppError> {
    let category = category_of(query.category.as_deref())?;
    let store = state.read();
    Ok(Json(player_info(&store, Mode::Alt, &login, category)?))
}

#[utoipa::path(
    get,
    path = "/players/{login}/matchups",
    tag = "players",
    params(
        ("login" = String, Path, description = "Player login (alt accounts resolve to their main)"),
        PageQuery
    ),
    responses(
        (status = 200, description = "Per-opponent kills and deaths", body = MatchupPage),
        (status = 400, description = "Invalid pagination or category", body = crate::error::ErrorResponse),
        (status = 404, description = "Unknown player", body = crate::error::ErrorResponse)
    )
)]
pub async fn matchups(
    State(state): State<AppState>,
    Path(login): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Json<MatchupPage>, AppError> {
    let category = category_of(query.category.as_deref())?;
    let page = Pagination::new(query.page, query.limit, &state.config)?;
    let store = state.read();
    Ok(Json(matchup_page(
        &store,
        Mode::Alt,
        &login,
        category,
        &page,
    )?))
}

#[utoipa::path(
    get,
    path = "/players/{login}/matchups/{opponent}",
    tag = "players",
    params(
        ("login" = String, Path, description = "Player login"),
        ("opponent" = String, Path, description = "Opponent login"),
        CategoryQuery
    ),
    responses(
        (status = 200, description = "Head-to-head record", body = HeadToHead),
        (status = 400, description = "Invalid category", body = crate::error::ErrorResponse),
        (status = 404, description = "Unknown player", body = crate::error::ErrorResponse)
    )
)]
pub async fn head_to_head_alt(
    State(state): State<AppState>,
    Path((login, opponent)): Path<(String, String)>,
    Query(query): Query<CategoryQuery>,
) -> Result<Json<HeadToHead>, AppError> {
    let category = category_of(query.category.as_deref())?;
    let store = state.read();
    Ok(Json(head_to_head(
        &store,
        Mode::Alt,
        &login,
        &opponent,
        category,
    )?))
}

#[utoipa::path(
    get,
    path = "/events",
    tag = "events",
    params(EventsQuery),
    responses(
        (status = 200, description = "Latest registered events first", body = EventPage),
        (status = 400, description = "Invalid pagination", body = crate::error::ErrorResponse),
        (status = 404, description = "Unknown player filter", body = crate::error::ErrorResponse)
    )
)]
pub async fn events(
    State(state): State<AppState>,
    Query(query): Query<EventsQuery>,
) -> Result<Json<EventPage>, AppError> {
    let page = Pagination::new(query.page, query.limit, &state.config)?;
    let store = state.read();
    Ok(Json(event_page(&store, Mode::Alt, &query, &page)?))
}

// --- alt-free endpoints -----------------------------------------------------

#[utoipa::path(
    get,
    path = "/noalt/leaderboard",
    tag = "leaderboard (no alt)",
    params(CategoryQuery),
    responses(
        (status = 200, description = "Complete ladder, every login separate", body = Vec<LeaderboardEntry>),
        (status = 400, description = "Invalid category", body = crate::error::ErrorResponse)
    )
)]
pub async fn noalt_leaderboard(
    State(state): State<AppState>,
    Query(query): Query<CategoryQuery>,
) -> Result<Json<Vec<LeaderboardEntry>>, AppError> {
    let category = category_of(query.category.as_deref())?;
    let store = state.read();
    Ok(Json(full_leaderboard(&store, Mode::NoAlt, category)))
}

#[utoipa::path(
    get,
    path = "/noalt/leaderboard/paginated",
    tag = "leaderboard (no alt)",
    params(PageQuery),
    responses(
        (status = 200, description = "One page of the ladder, every login separate", body = LeaderboardPage),
        (status = 400, description = "Invalid pagination or category", body = crate::error::ErrorResponse)
    )
)]
pub async fn noalt_leaderboard_paginated(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Json<LeaderboardPage>, AppError> {
    let category = category_of(query.category.as_deref())?;
    let page = Pagination::new(query.page, query.limit, &state.config)?;
    let store = state.read();
    Ok(Json(paged_leaderboard(
        &store,
        Mode::NoAlt,
        category,
        &page,
    )))
}

#[utoipa::path(
    get,
    path = "/noalt/players/{login}",
    tag = "players (no alt)",
    params(("login" = String, Path, description = "Player login, taken literally"), CategoryQuery),
    responses(
        (status = 200, description = "Player statistics", body = PlayerInfo),
        (status = 400, description = "Invalid category", body = crate::error::ErrorResponse),
        (status = 404, description = "Unknown player", body = crate::error::ErrorResponse)
    )
)]
pub async fn noalt_player(
    State(state): State<AppState>,
    Path(login): Path<String>,
    Query(query): Query<CategoryQuery>,
) -> Result<Json<PlayerInfo>, AppError> {
    let category = category_of(query.category.as_deref())?;
    let store = state.read();
    Ok(Json(player_info(&store, Mode::NoAlt, &login, category)?))
}

#[utoipa::path(
    get,
    path = "/noalt/players/{login}/matchups",
    tag = "players (no alt)",
    params(("login" = String, Path, description = "Player login, taken literally"), PageQuery),
    responses(
        (status = 200, description = "Per-opponent kills and deaths", body = MatchupPage),
        (status = 400, description = "Invalid pagination or category", body = crate::error::ErrorResponse),
        (status = 404, description = "Unknown player", body = crate::error::ErrorResponse)
    )
)]
pub async fn noalt_matchups(
    State(state): State<AppState>,
    Path(login): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Json<MatchupPage>, AppError> {
    let category = category_of(query.category.as_deref())?;
    let page = Pagination::new(query.page, query.limit, &state.config)?;
    let store = state.read();
    Ok(Json(matchup_page(
        &store,
        Mode::NoAlt,
        &login,
        category,
        &page,
    )?))
}

#[utoipa::path(
    get,
    path = "/noalt/players/{login}/matchups/{opponent}",
    tag = "players (no alt)",
    params(
        ("login" = String, Path, description = "Player login"),
        ("opponent" = String, Path, description = "Opponent login"),
        CategoryQuery
    ),
    responses(
        (status = 200, description = "Head-to-head record", body = HeadToHead),
        (status = 400, description = "Invalid category", body = crate::error::ErrorResponse),
        (status = 404, description = "Unknown player", body = crate::error::ErrorResponse)
    )
)]
pub async fn noalt_head_to_head(
    State(state): State<AppState>,
    Path((login, opponent)): Path<(String, String)>,
    Query(query): Query<CategoryQuery>,
) -> Result<Json<HeadToHead>, AppError> {
    let category = category_of(query.category.as_deref())?;
    let store = state.read();
    Ok(Json(head_to_head(
        &store,
        Mode::NoAlt,
        &login,
        &opponent,
        category,
    )?))
}

#[utoipa::path(
    get,
    path = "/noalt/events",
    tag = "events (no alt)",
    params(EventsQuery),
    responses(
        (status = 200, description = "Latest registered events first", body = EventPage),
        (status = 400, description = "Invalid pagination", body = crate::error::ErrorResponse),
        (status = 404, description = "Unknown player filter", body = crate::error::ErrorResponse)
    )
)]
pub async fn noalt_events(
    State(state): State<AppState>,
    Query(query): Query<EventsQuery>,
) -> Result<Json<EventPage>, AppError> {
    let page = Pagination::new(query.page, query.limit, &state.config)?;
    let store = state.read();
    Ok(Json(event_page(&store, Mode::NoAlt, &query, &page)?))
}
