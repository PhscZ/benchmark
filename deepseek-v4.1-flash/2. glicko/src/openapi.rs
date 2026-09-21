//! Route table and OpenAPI document (served by Swagger UI at `/swagger-ui`).

use crate::api::{self, AppState};
use crate::error::ErrorResponse;
use crate::model::{AltEntry, Category, EventInput};
use crate::store::ImportReport;
use axum::Router;
use axum::routing::{delete, get, post};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Glicko-2 kill-rating API",
        description = "Computes Glicko-2 ratings from chronologically ordered kill events.\n\n\
            Three independent ladders are maintained per player: `total` (every eligible event), \
            `1v1` (events with exactly two players in the lobby) and `pub` (more than two). \
            A match is a race to 20 kills; the winner is decided by the 20th kill and only that \
            pair's race counters are reset. Ratings are updated with a margin-of-victory score \
            derived from the final kill counts.\n\n\
            Every endpoint except `/noalt/*` collapses alt accounts into their main login; the \
            `/noalt/*` endpoints treat each login as an independent player.",
        version = env!("CARGO_PKG_VERSION")
    ),
    paths(
        api::health,
        api::get_config,
        api::import_events,
        api::list_alts,
        api::add_alts,
        api::delete_alt,
        api::leaderboard,
        api::leaderboard_paginated,
        api::player,
        api::matchups,
        api::head_to_head_alt,
        api::events,
        api::noalt_leaderboard,
        api::noalt_leaderboard_paginated,
        api::noalt_player,
        api::noalt_matchups,
        api::noalt_head_to_head,
        api::noalt_events,
    ),
    components(schemas(
        EventInput,
        AltEntry,
        Category,
        ImportReport,
        ErrorResponse,
        api::LeaderboardEntry,
        api::LeaderboardPage,
        api::PlayerInfo,
        api::MatchupEntry,
        api::MatchupPage,
        api::HeadToHead,
        api::EventDto,
        api::EventPage,
        api::AltsResponse,
        api::AltsRequest,
        api::HealthResponse,
        api::ConfigResponse,
    )),
    tags(
        (name = "service", description = "Health and effective configuration"),
        (name = "events", description = "Event import and event feed"),
        (name = "leaderboard", description = "Alt-aware ladders"),
        (name = "leaderboard (no alt)", description = "Ladders where every login stands alone"),
        (name = "players", description = "Alt-aware player statistics"),
        (name = "players (no alt)", description = "Player statistics where every login stands alone"),
        (name = "events (no alt)", description = "Event feed where every login stands alone"),
        (name = "alts", description = "Alt-account mappings and recalculation"),
    )
)]
pub struct ApiDoc;

/// Builds the full application router, including Swagger UI.
pub fn router(state: AppState) -> Router {
    let max_body = state.config.max_body_bytes;
    let routes = Router::new()
        .route("/health", get(api::health))
        .route("/config", get(api::get_config))
        .route("/events", get(api::events))
        .route("/events/import", post(api::import_events))
        .route("/leaderboard", get(api::leaderboard))
        .route("/leaderboard/paginated", get(api::leaderboard_paginated))
        .route("/players/{login}", get(api::player))
        .route("/players/{login}/matchups", get(api::matchups))
        .route(
            "/players/{login}/matchups/{opponent}",
            get(api::head_to_head_alt),
        )
        .route("/noalt/events", get(api::noalt_events))
        .route("/noalt/leaderboard", get(api::noalt_leaderboard))
        .route(
            "/noalt/leaderboard/paginated",
            get(api::noalt_leaderboard_paginated),
        )
        .route("/noalt/players/{login}", get(api::noalt_player))
        .route("/noalt/players/{login}/matchups", get(api::noalt_matchups))
        .route(
            "/noalt/players/{login}/matchups/{opponent}",
            get(api::noalt_head_to_head),
        )
        .route("/alts", get(api::list_alts).post(api::add_alts))
        .route("/alts/{alt}", delete(api::delete_alt))
        .layer(axum::extract::DefaultBodyLimit::max(max_body))
        .with_state(state);

    Router::new()
        .merge(routes)
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
}
